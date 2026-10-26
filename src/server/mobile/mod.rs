//! Opt-in mobile gateway: a WebSocket listener for the flockr phone app.
//!
//! Contract: `docs/mobile/protocol.md` (`mobile.v2`). The gateway runs as
//! tokio tasks on the server runtime plus a few dedicated threads (surface
//! drains, push worker, Expo sender). It costs nothing when disabled and adds
//! no work to the server loop until a phone subscribes to a surface.

pub(crate) mod agents;
pub(crate) mod api_proxy;
pub(crate) mod cells;
pub(crate) mod connection;
pub(crate) mod listener;
pub(crate) mod noise;
pub(crate) mod protocol;
pub(crate) mod push;
pub(crate) mod surface;

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use crate::config::MobileConfig;
use crate::mobile::store::{self, DevicesFile};
use crate::server::client_transport::ServerEvent;

pub(crate) const MAX_CONNECTIONS_PER_DEVICE: usize = 4;
pub(crate) const MAX_CONNECTIONS: usize = 16;
pub(crate) const MAX_UNAUTHENTICATED: usize = 8;
pub(crate) const MAX_UNAUTHENTICATED_PER_IP: usize = 3;
/// Deadline for the WebSocket upgrade and `hello` together.
pub(crate) const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const AUTH_FAILURES_PER_IP: usize = 5;
pub(crate) const AUTH_FAILURE_WINDOW: Duration = Duration::from_secs(60);
pub(crate) const PAIR_ATTEMPTS: usize = 10;
pub(crate) const PAIR_ATTEMPT_WINDOW: Duration = Duration::from_secs(60);
pub(crate) const MAX_SUBSCRIPTIONS: usize = 4;
pub(crate) const MAX_OPEN_REQUESTS: usize = 32;
pub(crate) const MAX_INBOUND_PER_SECOND: usize = 200;
pub(crate) const MAX_INPUT_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub(crate) const MAX_OUTBOUND_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_SEND_BUFFER_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_COLS: u16 = 500;
pub(crate) const MAX_ROWS: u16 = 200;
pub(crate) const PUSHES_PER_DEVICE_PER_MINUTE: usize = 30;
pub(crate) const REVOCATION_CHECK: Duration = Duration::from_secs(2);
pub(crate) const BIND_RETRY: Duration = Duration::from_secs(10);
pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(20);
pub(crate) const PONG_TIMEOUT: Duration = Duration::from_secs(60);
/// One outbound frame may take this long to write before the connection closes.
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// A foreground connection counts as live only if the phone sent something
/// (a pong answers every ping) within this window.
pub(crate) const FOREGROUND_FRESH: Duration = Duration::from_secs(25);
pub(crate) const LAST_SEEN_PERSIST: Duration = Duration::from_secs(600);
/// Mobile surface client ids live far above socket client ids.
const SURFACE_CLIENT_ID_BASE: u64 = 1 << 52;

/// Reply to `ServerEvent::MobileSurfaceOpen`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SurfaceOpened {
    pub pane_id: String,
    pub terminal_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceOpenError {
    NotFound,
    Busy,
    Unavailable,
}

/// Sliding-window counter (pure; `now` is passed in).
#[derive(Debug, Default)]
pub(crate) struct RateWindow {
    hits: VecDeque<Instant>,
}

impl RateWindow {
    /// Records a hit unless `limit` hits already fell inside `window`.
    pub(crate) fn try_hit(&mut self, now: Instant, limit: usize, window: Duration) -> bool {
        self.expire(now, window);
        if self.hits.len() >= limit {
            return false;
        }
        self.hits.push_back(now);
        true
    }

    pub(crate) fn count(&mut self, now: Instant, window: Duration) -> usize {
        self.expire(now, window);
        self.hits.len()
    }

    pub(crate) fn hit(&mut self, now: Instant, window: Duration) {
        self.expire(now, window);
        self.hits.push_back(now);
    }

    fn expire(&mut self, now: Instant, window: Duration) {
        while self
            .hits
            .front()
            .is_some_and(|hit| now.saturating_duration_since(*hit) >= window)
        {
            self.hits.pop_front();
        }
    }
}

/// A close request: send `error` (if any) then close with `code`.
pub(crate) type CloseRequest = Option<(u16, Option<String>)>;

/// Cloneable sender into one connection's outbound queue.
///
/// Closes go out of band (a `watch` slot the writer checks first), so a
/// revoked or slow connection closes without draining queued text.
#[derive(Clone, Debug)]
pub(crate) struct Outbound {
    tx: mpsc::UnboundedSender<String>,
    close_tx: Arc<watch::Sender<CloseRequest>>,
    queued: Arc<AtomicUsize>,
}

/// The writer side of an [`Outbound`].
pub(crate) struct OutboundRx {
    pub text: mpsc::UnboundedReceiver<String>,
    pub close: watch::Receiver<CloseRequest>,
    pub queued: Arc<AtomicUsize>,
}

impl Outbound {
    pub(crate) fn new() -> (Self, OutboundRx) {
        let (tx, text) = mpsc::unbounded_channel();
        let (close_tx, close) = watch::channel(None);
        let queued = Arc::new(AtomicUsize::new(0));
        (
            Self {
                tx,
                close_tx: Arc::new(close_tx),
                queued: queued.clone(),
            },
            OutboundRx {
                text,
                close,
                queued,
            },
        )
    }

    /// Queues a text frame. `false` when the connection is gone or too far behind.
    pub(crate) fn send(&self, text: String) -> bool {
        let len = text.len();
        let queued = self.queued.fetch_add(len, Ordering::AcqRel) + len;
        if queued > MAX_SEND_BUFFER_BYTES {
            self.queued.fetch_sub(len, Ordering::AcqRel);
            self.close(
                protocol::close::PROTOCOL_ERROR,
                Some(protocol::error(
                    None,
                    protocol::ErrorCode::BadRequest,
                    "client too slow",
                )),
            );
            return false;
        }
        self.tx.send(text).is_ok()
    }

    /// Asks the writer to close now, skipping queued text. The first close wins.
    pub(crate) fn close(&self, code: u16, error: Option<String>) {
        self.close_tx.send_if_modified(|slot| {
            if slot.is_some() {
                return false;
            }
            *slot = Some((code, error));
            true
        });
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

pub(crate) struct ConnEntry {
    pub device_id: String,
    pub out: Outbound,
    pub foreground: bool,
    /// Last time the phone sent anything on this connection.
    pub last_inbound: Instant,
}

#[derive(Default)]
pub(crate) struct GatewayState {
    pub conns: HashMap<u64, ConnEntry>,
    pub unauthenticated: usize,
    pub unauthenticated_by_ip: HashMap<IpAddr, usize>,
    pub ip_failures: HashMap<IpAddr, RateWindow>,
    pub pair_attempts: RateWindow,
    pub devices: DevicesFile,
    pub devices_mtime: Option<SystemTime>,
    pub last_seen_persisted: HashMap<String, Instant>,
}

/// Dependencies from the server process.
pub(crate) struct GatewayDeps {
    pub server_event_tx: mpsc::Sender<ServerEvent>,
    pub event_hub: crate::api::EventHub,
    pub api_socket: PathBuf,
}

/// Shared gateway state for every task and thread.
pub(crate) struct Gateway {
    pub dir: PathBuf,
    pub name: String,
    pub server_id: String,
    /// Noise static private key. Read-only; each connection builds its own
    /// handshake from it.
    pub noise_key: [u8; 32],
    pub boot_id: String,
    pub push: crate::config::MobilePushConfig,
    pub max_devices: u32,
    pub server_event_tx: mpsc::Sender<ServerEvent>,
    pub event_hub: crate::api::EventHub,
    pub api: api_proxy::ApiCaller,
    pub shutdown: watch::Receiver<bool>,
    pub stopped: Arc<AtomicBool>,
    next_surface_client_id: AtomicU64,
    next_conn_id: AtomicU64,
    state: Mutex<GatewayState>,
    pair_lock: Mutex<()>,
    bound: Mutex<Vec<String>>,
    pub push_queue: Option<push::ExpoQueue>,
}

impl Gateway {
    pub(crate) fn state(&self) -> MutexGuard<'_, GatewayState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn pair_lock(&self) -> MutexGuard<'_, ()> {
        self.pair_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn next_surface_client_id(&self) -> u64 {
        SURFACE_CLIENT_ID_BASE + self.next_surface_client_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn next_conn_id(&self) -> u64 {
        self.next_conn_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Reloads `devices.json` when its modification time changed. Returns
    /// whether it reloaded.
    pub(crate) fn refresh_devices(&self) -> bool {
        let mtime = store::devices_modified(&self.dir);
        let mut state = self.state();
        if state.devices_mtime == mtime && mtime.is_some() {
            return false;
        }
        state.devices = store::load_devices(&self.dir);
        state.devices_mtime = mtime;
        true
    }

    /// Writes `devices.json` and refreshes the cache from the result.
    pub(crate) fn update_devices<T>(
        &self,
        change: impl FnOnce(&mut DevicesFile) -> T,
    ) -> std::io::Result<T> {
        let (result, devices) = store::update_devices(&self.dir, change)?;
        let mut state = self.state();
        state.devices = devices;
        state.devices_mtime = store::devices_modified(&self.dir);
        Ok(result)
    }

    pub(crate) fn add_bound(&self, addr: String) {
        let mut bound = self
            .bound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !bound.contains(&addr) {
            bound.push(addr);
        }
        let record = store::GatewayRecord {
            version: 1,
            pid: std::process::id(),
            bound: bound.clone(),
            started_at: store::now_unix(),
        };
        if let Err(err) = store::save_gateway(&self.dir, &record) {
            warn!(err = %err, "could not write mobile gateway.json");
        }
    }

    pub(crate) fn features(&self) -> Vec<&'static str> {
        let mut features = vec![
            "api",
            "agents.watch",
            "surface",
            "surface.history",
            "surface.shift",
            "input",
        ];
        if self.push.enabled {
            features.push("push.expo");
        }
        features
    }

    /// Connections of `device_id` that reported foreground and sent something
    /// recently. A suspended app may never send `app.state` background, so a
    /// silent connection does not count.
    pub(crate) fn device_foreground_outbounds(&self, device_id: &str) -> Vec<Outbound> {
        let now = Instant::now();
        self.state()
            .conns
            .values()
            .filter(|conn| {
                conn.device_id == device_id
                    && conn.foreground
                    && now.saturating_duration_since(conn.last_inbound) < FOREGROUND_FRESH
            })
            .map(|conn| conn.out.clone())
            .collect()
    }

    /// Closes connections whose device record is gone.
    pub(crate) fn close_revoked(&self) {
        let state = self.state();
        for conn in state.conns.values() {
            if state.devices.find(&conn.device_id).is_none() {
                conn.out.close(
                    protocol::close::REVOKED,
                    Some(protocol::error(
                        None,
                        protocol::ErrorCode::Revoked,
                        "this device was removed from the server",
                    )),
                );
            }
        }
    }
}

/// Keeps the gateway running; dropping it stops the gateway.
pub(crate) struct MobileGatewayHandle {
    shutdown: watch::Sender<bool>,
    stopped: Arc<AtomicBool>,
    dir: PathBuf,
}

impl Drop for MobileGatewayHandle {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.shutdown.send(true);
        store::remove_gateway_if_owned(&self.dir, std::process::id());
    }
}

/// Starts the gateway when `[mobile]` enables it. Must run inside the tokio
/// runtime.
pub(crate) fn start(config: &MobileConfig, deps: GatewayDeps) -> Option<MobileGatewayHandle> {
    if !config.enabled {
        return None;
    }
    let (addrs, diagnostics) = config.listen_addrs();
    for diagnostic in &diagnostics {
        warn!("{diagnostic}");
    }
    if addrs.is_empty() {
        warn!("mobile gateway enabled but no usable listen address; not listening");
        return None;
    }
    let dir = store::mobile_dir();
    let server_id = match store::load_or_create_server_fingerprint(&dir) {
        Ok(server_id) => server_id,
        Err(err) => {
            warn!(err = %err, "mobile gateway could not create its server id; not listening");
            return None;
        }
    };
    let noise_key = match store::load_or_create_noise_key(&dir) {
        Ok(key) => key,
        Err(err) => {
            warn!(err = %err, "mobile gateway could not load its noise key; not listening");
            return None;
        }
    };
    let boot_id = store::random_bytes::<8>()
        .map(|bytes| format!("b-{}", store::base64url(&bytes)))
        .unwrap_or_else(|_| format!("b-{}", std::process::id()));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let stopped = Arc::new(AtomicBool::new(false));
    let api = api_proxy::ApiCaller::new(deps.api_socket);
    let push_queue = config.push.enabled.then(|| {
        push::ExpoQueue::start(config.push.access_token().map(str::to_owned), dir.clone())
    });
    let gateway = Arc::new(Gateway {
        dir: dir.clone(),
        name: config.display_name(),
        server_id,
        noise_key,
        boot_id,
        push: config.push.clone(),
        max_devices: config.max_devices,
        server_event_tx: deps.server_event_tx,
        event_hub: deps.event_hub,
        api,
        shutdown: shutdown_rx,
        stopped: stopped.clone(),
        next_surface_client_id: AtomicU64::new(1),
        next_conn_id: AtomicU64::new(1),
        state: Mutex::new(GatewayState::default()),
        pair_lock: Mutex::new(()),
        bound: Mutex::new(Vec::new()),
        push_queue,
    });
    gateway.refresh_devices();
    // Record the process even before a bind succeeds so the CLI can tell
    // "not running" from "still binding".
    if let Err(err) = store::save_gateway(
        &dir,
        &store::GatewayRecord {
            version: 1,
            pid: std::process::id(),
            bound: Vec::new(),
            started_at: store::now_unix(),
        },
    ) {
        warn!(err = %err, "could not write mobile gateway.json");
    }
    for addr in addrs {
        tokio::spawn(listener::run(gateway.clone(), addr));
    }
    tokio::spawn(revocation_loop(gateway.clone()));
    push::start_worker(gateway.clone());
    info!(name = %gateway.name, "mobile gateway started");
    Some(MobileGatewayHandle {
        shutdown: shutdown_tx,
        stopped,
        dir,
    })
}

async fn revocation_loop(gateway: Arc<Gateway>) {
    let mut shutdown = gateway.shutdown.clone();
    let mut tick = tokio::time::interval(REVOCATION_CHECK);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                // Other paths refresh the cache too, so compare live
                // connections against it on every tick, not only on reload.
                gateway.refresh_devices();
                gateway.close_revoked();
            }
            _ = shutdown.changed() => break,
        }
    }
}

#[cfg(test)]
impl Gateway {
    /// A gateway with no listener, server loop, or push threads.
    pub(crate) fn for_test(dir: PathBuf) -> Arc<Self> {
        let (server_event_tx, _) = mpsc::channel(1);
        let (shutdown_tx, shutdown) = watch::channel(false);
        // Keep the sender alive so connections in tests do not see a shutdown.
        std::mem::forget(shutdown_tx);
        let server_id = store::load_or_create_server_fingerprint(&dir).unwrap();
        let noise_key = store::load_or_create_noise_key(&dir).unwrap();
        Arc::new(Self {
            api: api_proxy::ApiCaller::new(dir.join("no-api.sock")),
            dir,
            name: "test".to_owned(),
            server_id,
            noise_key,
            boot_id: "b-test".to_owned(),
            push: crate::config::MobilePushConfig::default(),
            max_devices: 8,
            server_event_tx,
            event_hub: crate::api::EventHub::default(),
            shutdown,
            stopped: Arc::new(AtomicBool::new(false)),
            next_surface_client_id: AtomicU64::new(1),
            next_conn_id: AtomicU64::new(1),
            state: Mutex::new(GatewayState::default()),
            pair_lock: Mutex::new(()),
            bound: Mutex::new(Vec::new()),
            push_queue: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_window_limits_and_expires() {
        let mut window = RateWindow::default();
        let start = Instant::now();
        for _ in 0..5 {
            assert!(window.try_hit(start, 5, Duration::from_secs(60)));
        }
        assert!(!window.try_hit(start, 5, Duration::from_secs(60)));
        assert_eq!(
            window.count(start + Duration::from_secs(59), Duration::from_secs(60)),
            5
        );
        assert!(window.try_hit(start + Duration::from_secs(60), 5, Duration::from_secs(60)));
    }

    #[test]
    fn outbound_closes_when_the_send_buffer_overflows() {
        let (out, mut rx) = Outbound::new();
        assert!(out.send("x".repeat(MAX_SEND_BUFFER_BYTES - 1)));
        assert!(!out.send("yy".to_owned()));
        assert!(rx.text.try_recv().is_ok());
        assert!(rx.text.try_recv().is_err());
        assert!(rx.close.has_changed().unwrap());
        assert!(matches!(
            *rx.close.borrow_and_update(),
            Some((protocol::close::PROTOCOL_ERROR, _))
        ));
        assert_eq!(rx.queued.load(Ordering::Acquire), MAX_SEND_BUFFER_BYTES - 1);
    }

    #[test]
    fn close_is_out_of_band_and_first_close_wins() {
        let (out, mut rx) = Outbound::new();
        assert!(out.send("backlog".to_owned()));
        out.close(protocol::close::REVOKED, None);
        out.close(protocol::close::NORMAL, None);
        assert!(rx.close.has_changed().unwrap());
        assert!(matches!(
            *rx.close.borrow_and_update(),
            Some((protocol::close::REVOKED, None))
        ));
    }

    #[test]
    fn revocation_closes_connections_even_when_the_cache_is_already_fresh() {
        let dir = store::tests::temp_dir("gw-revoke");
        let gateway = Gateway::for_test(dir.clone());
        let (out, mut rx) = Outbound::new();
        gateway.state().conns.insert(
            1,
            ConnEntry {
                device_id: "d_gone".to_owned(),
                out,
                foreground: false,
                last_inbound: Instant::now(),
            },
        );
        // Another path (here a write) already refreshed the cache, so the
        // tick's refresh reports no change.
        gateway.update_devices(|_| ()).unwrap();
        assert!(!gateway.refresh_devices());
        gateway.close_revoked();
        assert!(matches!(
            *rx.close.borrow_and_update(),
            Some((protocol::close::REVOKED, _))
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn silent_foreground_connections_do_not_suppress_pushes() {
        let dir = store::tests::temp_dir("gw-foreground");
        let gateway = Gateway::for_test(dir.clone());
        let (out, _rx) = Outbound::new();
        let entry = |last_inbound| ConnEntry {
            device_id: "d_a".to_owned(),
            out: out.clone(),
            foreground: true,
            last_inbound,
        };
        gateway.state().conns.insert(1, entry(Instant::now()));
        assert_eq!(gateway.device_foreground_outbounds("d_a").len(), 1);
        if let Some(stale) = Instant::now().checked_sub(FOREGROUND_FRESH + Duration::from_secs(1)) {
            gateway.state().conns.insert(1, entry(stale));
            assert!(gateway.device_foreground_outbounds("d_a").is_empty());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
