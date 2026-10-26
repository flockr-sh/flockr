//! One phone connection: WebSocket upgrade, Noise handshake, `hello`, auth or
//! pairing, then message dispatch until either side closes (§7–§9).
//!
//! The Noise session lives in this task only: inbound frames are decrypted
//! and outbound messages encrypted here, never under a shared lock.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, info, warn};

use super::noise::{self, OpenError, Session};
use super::protocol::{self, close, ClientMessage, DecodeError, ErrorCode, Hello, InputKind};
use super::surface::{self, SubCtl, SubHandle};
use super::{
    agents, api_proxy, ConnEntry, Gateway, Outbound, OutboundRx, AUTH_FAILURES_PER_IP,
    AUTH_FAILURE_WINDOW, HELLO_TIMEOUT, LAST_SEEN_PERSIST, MAX_COLS, MAX_CONNECTIONS,
    MAX_CONNECTIONS_PER_DEVICE, MAX_INBOUND_PER_SECOND, MAX_INPUT_BYTES, MAX_MESSAGE_BYTES,
    MAX_OPEN_REQUESTS, MAX_ROWS, MAX_SUBSCRIPTIONS, MAX_UNAUTHENTICATED,
    MAX_UNAUTHENTICATED_PER_IP, PAIR_ATTEMPTS, PAIR_ATTEMPT_WINDOW, PING_INTERVAL, PONG_TIMEOUT,
    WRITE_TIMEOUT,
};
use crate::mobile::pairing::{self, PairError, PairRequest};
use crate::mobile::store::{self, DeviceRecord, PushRecord};
use crate::protocol::ClientPaneInputEvent;
use crate::server::client_transport::ServerEvent;

type Ws = WebSocketStream<TcpStream>;
type WsSink = futures_util::stream::SplitSink<Ws, Message>;
type WsSource = futures_util::stream::SplitStream<Ws>;

const MAX_UPGRADE_HEADER_BYTES: usize = 8 * 1024;
/// Each write while closing gets this long; the socket is dropped after.
const CLOSE_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a relative scroll keeps building on the last requested offset
/// instead of the offset in the last frame sent.
const SCROLL_TARGET_HOLD: Duration = Duration::from_secs(1);
/// Cap on one reassembled message after `hello`: the largest input message
/// plus room for its JSON envelope. Per-type limits are checked after decode.
const MAX_INBOUND_MESSAGE_BYTES: usize = MAX_INPUT_BYTES + 64 * 1024;

/// Upgrade check: path `/mobile/v2` and subprotocol `flockr.mobile.v2`.
#[allow(clippy::result_large_err)] // The error type is fixed by tungstenite's callback trait.
fn check_upgrade(request: &Request, mut response: Response) -> Result<Response, ErrorResponse> {
    let reject = |status: StatusCode, body: &str| {
        let mut error = ErrorResponse::new(Some(body.to_owned()));
        *error.status_mut() = status;
        error
    };
    if request.uri().path() != protocol::PATH {
        return Err(reject(StatusCode::NOT_FOUND, "not found"));
    }
    let header_bytes: usize = request
        .headers()
        .iter()
        .map(|(name, value)| name.as_str().len() + value.len() + 4)
        .sum();
    if header_bytes > MAX_UPGRADE_HEADER_BYTES {
        return Err(reject(
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            "headers too large",
        ));
    }
    let offered = request
        .headers()
        .get_all("Sec-WebSocket-Protocol")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|protocol| protocol.trim() == protocol::SUBPROTOCOL);
    if !offered {
        return Err(reject(
            StatusCode::BAD_REQUEST,
            &format!("missing subprotocol {}", protocol::SUBPROTOCOL),
        ));
    }
    response.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(protocol::SUBPROTOCOL),
    );
    Ok(response)
}

/// Sends one frame. `false` when the peer is gone or stopped reading.
async fn send_timed(sink: &mut WsSink, message: Message, limit: Duration) -> bool {
    matches!(
        tokio::time::timeout(limit, sink.send(message)).await,
        Ok(Ok(()))
    )
}

/// Sends a plaintext WebSocket close frame.
async fn close_plain(sink: &mut WsSink, code: u16) {
    let close = Message::Close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: "".into(),
    }));
    send_timed(sink, close, CLOSE_WRITE_TIMEOUT).await;
}

/// Write side of an established Noise channel.
struct Link {
    sink: WsSink,
    session: Session,
}

impl Link {
    /// Encrypts one JSON message and sends its frames. `false` when the peer
    /// is gone or stopped reading.
    async fn send(&mut self, text: &str, limit: Duration) -> bool {
        let frames = match self.session.seal(text.as_bytes()) {
            Ok(frames) => frames,
            Err(err) => {
                warn!(err = %err, "mobile message could not be encrypted");
                return false;
            }
        };
        let sink = &mut self.sink;
        let write = async move {
            for frame in frames {
                sink.feed(Message::Binary(frame.into())).await?;
            }
            sink.flush().await
        };
        matches!(tokio::time::timeout(limit, write).await, Ok(Ok(())))
    }

    /// Sends `error` (if any) through the channel, then a plaintext close.
    async fn close(&mut self, code: u16, error: Option<String>) {
        if let Some(error) = error {
            if !self.send(&error, CLOSE_WRITE_TIMEOUT).await {
                return;
            }
        }
        close_plain(&mut self.sink, code).await;
    }
}

fn open_error_code(err: OpenError) -> u16 {
    match err {
        OpenError::TooBig => close::TOO_BIG,
        OpenError::Decrypt | OpenError::BadFlag => close::PROTOCOL_ERROR,
    }
}

/// Holds one of the limited unauthenticated slots until dropped.
struct UnauthSlot {
    gateway: Arc<Gateway>,
    ip: IpAddr,
    held: bool,
}

impl UnauthSlot {
    /// `None` when the gateway or this IP already has too many connections
    /// waiting for `hello`.
    fn acquire(gateway: &Arc<Gateway>, ip: IpAddr) -> Option<Self> {
        let mut state = gateway.state();
        let per_ip = state.unauthenticated_by_ip.get(&ip).copied().unwrap_or(0);
        if state.unauthenticated >= MAX_UNAUTHENTICATED || per_ip >= MAX_UNAUTHENTICATED_PER_IP {
            return None;
        }
        state.unauthenticated += 1;
        *state.unauthenticated_by_ip.entry(ip).or_default() += 1;
        drop(state);
        Some(Self {
            gateway: gateway.clone(),
            ip,
            held: true,
        })
    }

    fn release(&mut self) {
        if self.held {
            self.held = false;
            let mut state = self.gateway.state();
            state.unauthenticated = state.unauthenticated.saturating_sub(1);
            if let Some(count) = state.unauthenticated_by_ip.get_mut(&self.ip) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    state.unauthenticated_by_ip.remove(&self.ip);
                }
            }
        }
    }
}

impl Drop for UnauthSlot {
    fn drop(&mut self) {
        self.release();
    }
}

fn ip_blocked(gateway: &Gateway, ip: IpAddr) -> bool {
    let now = Instant::now();
    gateway
        .state()
        .ip_failures
        .get_mut(&ip)
        .is_some_and(|window| window.count(now, AUTH_FAILURE_WINDOW) >= AUTH_FAILURES_PER_IP)
}

fn record_failure(gateway: &Gateway, ip: IpAddr) {
    gateway
        .state()
        .ip_failures
        .entry(ip)
        .or_default()
        .hit(Instant::now(), AUTH_FAILURE_WINDOW);
}

/// Who completed `hello`, and the token to return on the pair path.
struct Authenticated {
    device: DeviceRecord,
    token: Option<String>,
}

/// Error to send before closing, and the close code.
type Reject = (u16, String);

fn authenticate(gateway: &Gateway, hello: &Hello, ip: IpAddr) -> Result<Authenticated, Reject> {
    match (&hello.auth, &hello.pair) {
        (Some(auth), None) => {
            gateway.refresh_devices();
            let device = gateway
                .state()
                .devices
                .authenticate(&auth.device_id, &auth.token)
                .cloned();
            match device {
                Some(device) => Ok(Authenticated {
                    device,
                    token: None,
                }),
                None => {
                    record_failure(gateway, ip);
                    Err((
                        close::UNAUTHORIZED,
                        protocol::error(None, ErrorCode::Unauthorized, "device not recognized"),
                    ))
                }
            }
        }
        (None, Some(pair)) => {
            let allowed = gateway.state().pair_attempts.try_hit(
                Instant::now(),
                PAIR_ATTEMPTS,
                PAIR_ATTEMPT_WINDOW,
            );
            if !allowed {
                return Err((
                    close::RATE_LIMITED,
                    protocol::error(None, ErrorCode::RateLimited, "too many pairing attempts"),
                ));
            }
            let platform = hello.app.platform.as_deref().unwrap_or("unknown");
            let request = PairRequest {
                code: &pair.code,
                sid: &pair.sid,
                device_name: &pair.device_name,
                platform,
            };
            let result = {
                let _lock = gateway.pair_lock();
                pairing::redeem(
                    &gateway.dir,
                    &request,
                    &gateway.server_id,
                    gateway.max_devices,
                    store::now_unix(),
                )
            };
            match result {
                Ok(paired) => {
                    gateway.refresh_devices();
                    info!(device_id = %paired.device.device_id, name = %paired.device.name, "mobile device paired");
                    Ok(Authenticated {
                        device: paired.device,
                        token: Some(paired.token),
                    })
                }
                Err(PairError::TooManyDevices) => Err((
                    close::PAIRING_FAILED,
                    protocol::error(
                        None,
                        ErrorCode::TooManyDevices,
                        "this server has the maximum number of paired devices",
                    ),
                )),
                Err(PairError::Failed(reason)) => {
                    record_failure(gateway, ip);
                    debug!(reason, "mobile pairing failed");
                    Err((
                        close::PAIRING_FAILED,
                        protocol::error(
                            None,
                            ErrorCode::PairingFailed,
                            "pairing code is wrong, expired, or already used; run `flockr mobile pair` again",
                        ),
                    ))
                }
                Err(PairError::Io(err)) => {
                    warn!(err = %err, "mobile pairing could not write state");
                    Err((
                        close::PAIRING_FAILED,
                        protocol::error(
                            None,
                            ErrorCode::Internal,
                            "server could not save the pairing",
                        ),
                    ))
                }
            }
        }
        _ => Err((
            close::PROTOCOL_ERROR,
            protocol::error(
                None,
                ErrorCode::BadRequest,
                "hello needs exactly one of `auth` or `pair`",
            ),
        )),
    }
}

fn welcome(gateway: &Gateway, auth: &Authenticated, max_in_flight: u32) -> String {
    let mut device = json!({
        "device_id": auth.device.device_id,
        "name": auth.device.name,
    });
    if let Some(token) = &auth.token {
        device["token"] = json!(token);
    }
    json!({
        "t": "welcome",
        "version": protocol::VERSION,
        "server": {
            "name": gateway.name,
            "server_id": gateway.server_id,
            "flockr_version": env!("CARGO_PKG_VERSION"),
            "boot_id": gateway.boot_id,
        },
        "device": device,
        "features": gateway.features(),
        "methods": api_proxy::ALLOWED_METHODS,
        "limits": {
            "max_in_flight": max_in_flight,
            "max_subscriptions": MAX_SUBSCRIPTIONS,
            "max_message_bytes": MAX_MESSAGE_BYTES,
            "max_input_bytes": MAX_INPUT_BYTES,
            "max_cols": MAX_COLS,
            "max_rows": MAX_ROWS,
        }
    })
    .to_string()
}

/// Reads the next binary frame (skipping pings/pongs). `Err` carries a
/// close code.
async fn next_binary(source: &mut WsSource) -> Result<Option<Vec<u8>>, u16> {
    loop {
        match source.next().await {
            None => return Ok(None),
            Some(Ok(Message::Binary(frame))) => return Ok(Some(frame.to_vec())),
            Some(Ok(Message::Text(_))) => return Err(close::PROTOCOL_ERROR),
            Some(Ok(Message::Close(_))) => return Ok(None),
            Some(Ok(_)) => continue,
            Some(Err(WsError::Capacity(_))) => return Err(close::TOO_BIG),
            Some(Err(_)) => return Ok(None),
        }
    }
}

/// Reads frames until one message of at most `cap` bytes is complete.
async fn next_message(
    source: &mut WsSource,
    session: &mut Session,
    cap: usize,
) -> Result<Option<String>, u16> {
    loop {
        let Some(frame) = next_binary(source).await? else {
            return Ok(None);
        };
        match session.open(&frame, cap) {
            Ok(None) => continue,
            Ok(Some(bytes)) => {
                return String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| close::PROTOCOL_ERROR)
            }
            Err(err) => return Err(open_error_code(err)),
        }
    }
}

pub(crate) async fn serve(gateway: Arc<Gateway>, stream: TcpStream, peer: SocketAddr) {
    let ip = peer.ip();
    // Refuse before the upgrade so idle or blocked peers cost no task time
    // and release their socket at once.
    if ip_blocked(&gateway, ip) {
        debug!(%peer, "mobile connection refused: too many failures from this address");
        return;
    }
    let Some(mut slot) = UnauthSlot::acquire(&gateway, ip) else {
        debug!(%peer, "mobile connection refused: too many unauthenticated connections");
        return;
    };
    // One deadline covers the upgrade, the Noise handshake, and `hello`.
    let deadline = tokio::time::Instant::now() + HELLO_TIMEOUT;
    // Every frame is one Noise message, so no frame is larger than Noise's limit.
    let config = WebSocketConfig::default()
        .max_message_size(Some(noise::MAX_FRAME_LEN))
        .max_frame_size(Some(noise::MAX_FRAME_LEN));
    let upgrade =
        tokio_tungstenite::accept_hdr_async_with_config(stream, check_upgrade, Some(config));
    let ws = match tokio::time::timeout_at(deadline, upgrade).await {
        Ok(Ok(ws)) => ws,
        Ok(Err(err)) => {
            debug!(%peer, err = %err, "mobile upgrade failed");
            record_failure(&gateway, ip);
            return;
        }
        Err(_) => {
            record_failure(&gateway, ip);
            return;
        }
    };
    let (mut sink, mut source) = ws.split();

    let msg1 = match tokio::time::timeout_at(deadline, next_binary(&mut source)).await {
        Err(_) => {
            record_failure(&gateway, ip);
            close_plain(&mut sink, close::HELLO_TIMEOUT).await;
            return;
        }
        Ok(Err(code)) => {
            close_plain(&mut sink, code).await;
            return;
        }
        Ok(Ok(None)) => return,
        Ok(Ok(Some(frame))) => frame,
    };
    let accepted = noise::responder(&gateway.noise_key)
        .map_err(|err| err.to_string())
        .and_then(|builder| noise::accept(builder, &msg1));
    let (msg2, session) = match accepted {
        Ok(accepted) => accepted,
        Err(reason) => {
            debug!(%peer, reason, "mobile handshake failed");
            record_failure(&gateway, ip);
            close_plain(&mut sink, close::HANDSHAKE_FAILED).await;
            return;
        }
    };
    let sent = tokio::time::timeout_at(deadline, sink.send(Message::Binary(msg2.into()))).await;
    if !matches!(sent, Ok(Ok(()))) {
        return;
    }
    let mut link = Link { sink, session };

    let next = next_message(&mut source, &mut link.session, MAX_MESSAGE_BYTES);
    let hello_text = match tokio::time::timeout_at(deadline, next).await {
        Err(_) => {
            record_failure(&gateway, ip);
            close_plain(&mut link.sink, close::HELLO_TIMEOUT).await;
            return;
        }
        Ok(Err(code)) => {
            close_plain(&mut link.sink, code).await;
            return;
        }
        Ok(Ok(None)) => return,
        Ok(Ok(Some(text))) => text,
    };
    let hello = match ClientMessage::decode(&hello_text) {
        Ok(ClientMessage::Hello(hello)) => hello,
        Ok(_) | Err(DecodeError::Malformed(_)) => {
            link.close(
                close::PROTOCOL_ERROR,
                Some(protocol::error(
                    None,
                    ErrorCode::BadRequest,
                    "first message must be hello",
                )),
            )
            .await;
            return;
        }
        Err(DecodeError::BadRequest { message, .. }) => {
            link.close(
                close::PROTOCOL_ERROR,
                Some(protocol::error(None, ErrorCode::BadRequest, message)),
            )
            .await;
            return;
        }
    };
    if hello.negotiated_version().is_none() {
        let mut error: Value = serde_json::from_str(&protocol::error(
            None,
            ErrorCode::UnsupportedVersion,
            "no common protocol version",
        ))
        .unwrap_or_default();
        error["supported"] = json!([protocol::VERSION]);
        link.close(close::UNSUPPORTED_VERSION, Some(error.to_string()))
            .await;
        return;
    }
    let auth = match authenticate(&gateway, &hello, ip) {
        Ok(auth) => auth,
        Err((code, error)) => {
            link.close(code, Some(error)).await;
            return;
        }
    };
    let device_id = auth.device.device_id.clone();

    let (out, out_rx) = Outbound::new();
    let conn_id = gateway.next_conn_id();
    let registered = {
        let mut state = gateway.state();
        let device_conns = state
            .conns
            .values()
            .filter(|conn| conn.device_id == device_id)
            .count();
        let allowed =
            state.conns.len() < MAX_CONNECTIONS && device_conns < MAX_CONNECTIONS_PER_DEVICE;
        if allowed {
            state.conns.insert(
                conn_id,
                ConnEntry {
                    device_id: device_id.clone(),
                    out: out.clone(),
                    foreground: false,
                    last_inbound: Instant::now(),
                },
            );
        }
        allowed
    };
    if !registered {
        link.close(
            close::TOO_MANY_CONNECTIONS,
            Some(protocol::error(
                None,
                ErrorCode::TooManyConnections,
                "too many connections",
            )),
        )
        .await;
        return;
    }
    slot.release();
    touch_last_seen(&gateway, &device_id, false);

    let welcome = welcome(&gateway, &auth, hello.max_in_flight());
    if !link.send(&welcome, WRITE_TIMEOUT).await {
        gateway.state().conns.remove(&conn_id);
        return;
    }
    info!(%peer, device_id = %device_id, "mobile device connected");

    let (sub_ended_tx, sub_ended_rx) = mpsc::unbounded_channel();
    let (api_done_tx, api_done_rx) = mpsc::unbounded_channel();
    let mut conn = Conn {
        gateway: gateway.clone(),
        conn_id,
        device_id: device_id.clone(),
        out,
        max_in_flight: hello.max_in_flight(),
        subs: HashMap::new(),
        api: HashMap::new(),
        agents_watch: None,
        inbound: super::RateWindow::default(),
        last_rate_error: None,
        sub_ended_tx,
        api_done_tx,
    };
    conn.run(link, source, out_rx, sub_ended_rx, api_done_rx)
        .await;
    conn.cleanup();
    gateway.state().conns.remove(&conn_id);
    touch_last_seen(&gateway, &device_id, true);
    info!(%peer, device_id = %device_id, "mobile device disconnected");
}

fn touch_last_seen(gateway: &Gateway, device_id: &str, force: bool) {
    let now = Instant::now();
    let due = {
        let mut state = gateway.state();
        let due = force
            || state
                .last_seen_persisted
                .get(device_id)
                .is_none_or(|at| now.saturating_duration_since(*at) >= LAST_SEEN_PERSIST);
        if due {
            state.last_seen_persisted.insert(device_id.to_owned(), now);
        }
        due
    };
    if due {
        let seen = store::now_unix();
        let result = gateway.update_devices(|devices| {
            if let Some(device) = devices.find_mut(device_id) {
                device.last_seen_at = seen;
            }
        });
        if let Err(err) = result {
            warn!(err = %err, "could not record mobile device last-seen time");
        }
    }
}

enum Flow {
    Continue,
    Close(u16, Option<String>),
}

struct Conn {
    gateway: Arc<Gateway>,
    conn_id: u64,
    device_id: String,
    out: Outbound,
    max_in_flight: u32,
    subs: HashMap<u32, SubHandle>,
    api: HashMap<String, Arc<AtomicBool>>,
    agents_watch: Option<(String, oneshot::Sender<()>)>,
    inbound: super::RateWindow,
    last_rate_error: Option<Instant>,
    sub_ended_tx: mpsc::UnboundedSender<u32>,
    api_done_tx: mpsc::UnboundedSender<String>,
}

impl Conn {
    async fn run(
        &mut self,
        mut link: Link,
        mut source: WsSource,
        out_rx: OutboundRx,
        mut sub_ended_rx: mpsc::UnboundedReceiver<u32>,
        mut api_done_rx: mpsc::UnboundedReceiver<String>,
    ) {
        let OutboundRx {
            text: mut out_rx,
            close: mut close_rx,
            queued,
        } = out_rx;
        let mut shutdown = self.gateway.shutdown.clone();
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.tick().await;
        let mut last_pong = Instant::now();
        let mut last_alive_mark = Instant::now();
        loop {
            // Biased: a close request (revoked, too slow) wins over queued
            // output and inbound messages.
            tokio::select! {
                biased;
                changed = close_rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let request = close_rx.borrow_and_update().clone();
                    if let Some((code, error)) = request {
                        link.close(code, error).await;
                        return;
                    }
                }
                _ = shutdown.changed() => {
                    let bye = json!({ "t": "bye", "reason": "server_shutdown" }).to_string();
                    if link.send(&bye, CLOSE_WRITE_TIMEOUT).await {
                        close_plain(&mut link.sink, close::SERVER_SHUTDOWN).await;
                    }
                    return;
                }
                message = out_rx.recv() => match message {
                    Some(text) => {
                        let len = text.len();
                        let sent = link.send(&text, WRITE_TIMEOUT).await;
                        queued.fetch_sub(len, Ordering::AcqRel);
                        if !sent {
                            debug!(device_id = %self.device_id, "mobile write failed or stalled; closing");
                            return;
                        }
                    }
                    None => return,
                },
                incoming = source.next() => {
                    if incoming.is_some() && last_alive_mark.elapsed() >= Duration::from_secs(1) {
                        last_alive_mark = Instant::now();
                        if let Some(conn) = self.gateway.state().conns.get_mut(&self.conn_id) {
                            conn.last_inbound = last_alive_mark;
                        }
                    }
                    match incoming {
                    None | Some(Ok(Message::Close(_))) => return,
                    Some(Ok(Message::Binary(frame))) => {
                        let text = match link.session.open(&frame, MAX_INBOUND_MESSAGE_BYTES) {
                            Ok(None) => continue,
                            Ok(Some(bytes)) => String::from_utf8(bytes),
                            Err(err) => {
                                close_plain(&mut link.sink, open_error_code(err)).await;
                                return;
                            }
                        };
                        let flow = match text {
                            Ok(text) => self.handle_text(&text).await,
                            Err(_) => Flow::Close(
                                close::PROTOCOL_ERROR,
                                Some(protocol::error(None, ErrorCode::BadRequest, "message is not UTF-8")),
                            ),
                        };
                        if let Flow::Close(code, error) = flow {
                            link.close(code, error).await;
                            return;
                        }
                    }
                    Some(Ok(Message::Text(_))) => {
                        link.close(
                            close::PROTOCOL_ERROR,
                            Some(protocol::error(None, ErrorCode::BadRequest, "text frames are not supported")),
                        )
                        .await;
                        return;
                    }
                    Some(Ok(Message::Pong(_))) => last_pong = Instant::now(),
                    Some(Ok(_)) => {}
                    Some(Err(WsError::Capacity(_))) => {
                        close_plain(&mut link.sink, close::TOO_BIG).await;
                        return;
                    }
                    Some(Err(_)) => return,
                    }
                },
                _ = ping.tick() => {
                    if last_pong.elapsed() > PONG_TIMEOUT {
                        debug!(device_id = %self.device_id, "mobile connection missed pongs; closing");
                        return;
                    }
                    if !send_timed(&mut link.sink, Message::Ping(Vec::new().into()), WRITE_TIMEOUT).await {
                        return;
                    }
                }
                Some(sub) = sub_ended_rx.recv() => {
                    self.subs.remove(&sub);
                }
                Some(id) = api_done_rx.recv() => {
                    self.api.remove(&id);
                }
            }
        }
    }

    fn cleanup(&mut self) {
        for (_, sub) in self.subs.drain() {
            let _ = sub.ctl.send(SubCtl::Stop);
        }
        for (_, cancel) in self.api.drain() {
            cancel.store(true, Ordering::Release);
        }
        if let Some((_, stop)) = self.agents_watch.take() {
            let _ = stop.send(());
        }
        let _ = self.conn_id;
    }

    fn send(&self, text: String) {
        self.out.send(text);
    }

    fn error(&self, id: Option<&str>, code: ErrorCode, message: impl Into<String>) {
        self.send(protocol::error(id, code, message));
    }

    async fn handle_text(&mut self, text: &str) -> Flow {
        let now = Instant::now();
        if !self.inbound.try_hit(
            now,
            MAX_INBOUND_PER_SECOND,
            std::time::Duration::from_secs(1),
        ) {
            if self
                .last_rate_error
                .is_none_or(|at| now.saturating_duration_since(at).as_secs() >= 1)
            {
                self.last_rate_error = Some(now);
                self.error(None, ErrorCode::RateLimited, "too many messages; slow down");
            }
            return Flow::Continue;
        }
        let message = match ClientMessage::decode(text) {
            Ok(message) => message,
            Err(DecodeError::Malformed(message)) => {
                return Flow::Close(
                    close::PROTOCOL_ERROR,
                    Some(protocol::error(None, ErrorCode::BadRequest, message)),
                )
            }
            Err(DecodeError::BadRequest { id, message }) => {
                self.error(id.as_deref(), ErrorCode::BadRequest, message);
                return Flow::Continue;
            }
        };
        let is_input = matches!(message, ClientMessage::Input(_));
        if text.len() > MAX_MESSAGE_BYTES && !is_input {
            return Flow::Close(close::TOO_BIG, None);
        }
        match message {
            ClientMessage::Hello(_) => {
                self.error(None, ErrorCode::BadRequest, "already authenticated");
            }
            ClientMessage::Ping { ts } => {
                self.send(json!({ "t": "pong", "ts": ts }).to_string());
            }
            ClientMessage::ApiRequest(request) => self.api_request(request),
            ClientMessage::ApiCancel(request) => {
                if let Some(cancel) = self.api.get(&request.id) {
                    cancel.store(true, Ordering::Release);
                }
            }
            ClientMessage::AgentsWatch(request) => {
                if let Some((old, stop)) = self.agents_watch.take() {
                    let _ = stop.send(());
                    if old != request.id {
                        self.send(protocol::reply("api.end", &old));
                    }
                }
                let (stop_tx, stop_rx) = oneshot::channel();
                tokio::spawn(agents::watch(
                    self.gateway.clone(),
                    self.out.clone(),
                    request.id.clone(),
                    stop_rx,
                ));
                self.agents_watch = Some((request.id, stop_tx));
            }
            ClientMessage::AgentsUnwatch(request) => {
                if self
                    .agents_watch
                    .as_ref()
                    .is_some_and(|(id, _)| *id == request.id)
                {
                    if let Some((id, stop)) = self.agents_watch.take() {
                        let _ = stop.send(());
                        self.send(protocol::reply("api.end", &id));
                    }
                }
            }
            ClientMessage::SurfaceSubscribe(request) => {
                let id = request.id.clone();
                if self.subs.contains_key(&request.sub) {
                    self.error(
                        id.as_deref(),
                        ErrorCode::BadRequest,
                        "`sub` is already open",
                    );
                } else if self.subs.len() >= MAX_SUBSCRIPTIONS {
                    self.error(
                        id.as_deref(),
                        ErrorCode::TooManySubscriptions,
                        "too many open subscriptions",
                    );
                } else {
                    match surface::open(
                        &self.gateway,
                        &self.device_id,
                        &self.out,
                        self.sub_ended_tx.clone(),
                        &request,
                        self.max_in_flight,
                    )
                    .await
                    {
                        Ok(handle) => {
                            self.subs.insert(request.sub, handle);
                        }
                        Err((code, message)) => self.error(id.as_deref(), code, message),
                    }
                }
            }
            ClientMessage::SurfaceResize(request) => {
                let Some(sub) = self.subs.get(&request.sub) else {
                    self.error(
                        request.id.as_deref(),
                        ErrorCode::NotFound,
                        "no such subscription",
                    );
                    return Flow::Continue;
                };
                if !sub.fit {
                    self.error(
                        request.id.as_deref(),
                        ErrorCode::BadRequest,
                        "surface.resize needs a fit-mode subscription",
                    );
                    return Flow::Continue;
                }
                let _ = self
                    .gateway
                    .server_event_tx
                    .send(ServerEvent::ClientResize {
                        client_id: sub.client_id,
                        cols: request.cols.clamp(2, MAX_COLS),
                        rows: request.rows.clamp(2, MAX_ROWS),
                        cell_width_px: 0,
                        cell_height_px: 0,
                        pixel_mouse: false,
                    })
                    .await;
            }
            ClientMessage::SurfaceAck(ack) => {
                if let Some(sub) = self.subs.get(&ack.sub) {
                    let _ = sub.ctl.send(SubCtl::Ack(ack.seq));
                }
            }
            ClientMessage::SurfaceResync(request) => {
                if let Some(sub) = self.subs.get(&request.sub) {
                    let _ = sub.ctl.send(SubCtl::Resync);
                }
            }
            ClientMessage::SurfaceUnsubscribe(request) => {
                if let Some(sub) = self.subs.get(&request.sub) {
                    let _ = sub.ctl.send(SubCtl::Stop);
                }
            }
            ClientMessage::SurfaceScroll(request) => self.surface_scroll(request).await,
            ClientMessage::SurfaceHistory(request) => self.surface_history(request).await,
            ClientMessage::Input(input) => {
                let limit = match &input.kind {
                    InputKind::Key(_) => MAX_MESSAGE_BYTES,
                    _ => MAX_INPUT_BYTES,
                };
                if text.len() > limit + 1024 {
                    return Flow::Close(close::TOO_BIG, None);
                }
                self.input(input).await;
            }
            ClientMessage::PushRegister(request) => {
                let id = request.id.as_deref();
                if !self.gateway.push.enabled {
                    self.error(
                        id,
                        ErrorCode::PushUnavailable,
                        "push is disabled on this server",
                    );
                } else if !protocol::valid_expo_token(&request.expo_token) {
                    self.error(id, ErrorCode::BadRequest, "not an Expo push token");
                } else {
                    let token = request.expo_token.clone();
                    let result = self.gateway.update_devices(|devices| {
                        devices.find_mut(&self.device_id).map(|device| {
                            device.push = Some(PushRecord {
                                expo_token: token,
                                registered_at: store::now_unix(),
                                extra: serde_json::Map::new(),
                            });
                        })
                    });
                    match result {
                        Ok(Some(())) => {
                            if let Some(id) = id {
                                self.send(protocol::reply("push.ok", id));
                            }
                        }
                        Ok(None) => self.error(id, ErrorCode::Revoked, "device was removed"),
                        Err(err) => self.error(id, ErrorCode::Internal, err.to_string()),
                    }
                }
            }
            ClientMessage::PushUnregister { id } => {
                let result = self.gateway.update_devices(|devices| {
                    if let Some(device) = devices.find_mut(&self.device_id) {
                        device.push = None;
                    }
                });
                match result {
                    Ok(()) => {
                        if let Some(id) = id {
                            self.send(protocol::reply("push.ok", &id));
                        }
                    }
                    Err(err) => self.error(id.as_deref(), ErrorCode::Internal, err.to_string()),
                }
            }
            ClientMessage::AppState(state) => {
                if let Some(conn) = self.gateway.state().conns.get_mut(&self.conn_id) {
                    conn.foreground = state.foreground;
                }
            }
            ClientMessage::DeviceForget { id } => {
                let device_id = self.device_id.clone();
                let result = self
                    .gateway
                    .update_devices(|devices| devices.remove(&device_id));
                if let Err(err) = result {
                    self.error(id.as_deref(), ErrorCode::Internal, err.to_string());
                    return Flow::Continue;
                }
                info!(device_id = %self.device_id, "mobile device removed itself");
                let reply = id.map(|id| protocol::reply("device.ok", &id));
                // Other connections of this device close as revoked.
                self.gateway.close_revoked();
                return Flow::Close(close::NORMAL, reply);
            }
            ClientMessage::Unknown { t, id } => match id {
                Some(id) => self.error(
                    Some(&id),
                    ErrorCode::UnknownType,
                    format!("unknown message type {t}"),
                ),
                None => debug!(t, "ignored unknown mobile message"),
            },
        }
        Flow::Continue
    }

    fn api_request(&mut self, request: protocol::ApiRequest) {
        if !api_proxy::method_allowed(&request.method) {
            self.error(
                Some(&request.id),
                ErrorCode::MethodNotAllowed,
                format!("{} is not available from the phone", request.method),
            );
            return;
        }
        if self.api.contains_key(&request.id) {
            self.error(
                Some(&request.id),
                ErrorCode::BadRequest,
                "`id` is already in use",
            );
            return;
        }
        if self.api.len() >= MAX_OPEN_REQUESTS {
            self.error(
                Some(&request.id),
                ErrorCode::RateLimited,
                "too many open API requests",
            );
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.api.insert(request.id.clone(), cancel.clone());
        let api = self.gateway.api.clone();
        let out = self.out.clone();
        let done = self.api_done_tx.clone();
        tokio::task::spawn_blocking(move || {
            api.proxy(&request.id, &request.method, &request.params, &out, &cancel);
            let _ = done.send(request.id);
        });
    }

    /// One internal API call on a blocking thread.
    async fn call(
        &self,
        method: &'static str,
        params: Value,
    ) -> Result<Value, api_proxy::ApiCallError> {
        let api = self.gateway.api.clone();
        tokio::task::spawn_blocking(move || api.call(method, params))
            .await
            .unwrap_or_else(|err| Err(api_proxy::ApiCallError::Unavailable(err.to_string())))
    }

    fn api_error(&self, id: Option<&str>, err: api_proxy::ApiCallError) {
        let code = match &err {
            api_proxy::ApiCallError::Unavailable(_) => ErrorCode::ServerUnavailable,
            api_proxy::ApiCallError::Api { code, .. } => match code.as_str() {
                "invalid_key" | "invalid_request" | "invalid_params" => ErrorCode::BadRequest,
                code if code.contains("not_found") => ErrorCode::NotFound,
                _ => ErrorCode::Internal,
            },
        };
        if id.is_some() || matches!(code, ErrorCode::BadRequest | ErrorCode::NotFound) {
            self.error(id, code, err.to_string());
        } else {
            debug!(err = %err, "mobile request failed");
        }
    }

    async fn surface_scroll(&mut self, request: protocol::SurfaceScroll) {
        let Some(sub) = self.subs.get_mut(&request.sub) else {
            self.error(
                request.id.as_deref(),
                ErrorCode::NotFound,
                "no such subscription",
            );
            return;
        };
        let pane_id = sub.pane_id.clone();
        let (frame_offset, max) = sub
            .scroll
            .lock()
            .ok()
            .and_then(|state| *state)
            .unwrap_or((0, u64::MAX));
        // Frames lag behind requests (acks, max_fps), so quick relative
        // scrolls build on the last requested offset, not the last frame.
        let now = Instant::now();
        let offset = match sub.scroll_target {
            Some((target, at)) if now.saturating_duration_since(at) < SCROLL_TARGET_HOLD => target,
            _ => frame_offset,
        };
        let target = if request.to.as_deref() == Some("bottom") {
            0
        } else if let Some(lines) = request.lines {
            let next = i128::from(offset) + i128::from(lines);
            next.clamp(0, i128::from(max)) as u64
        } else {
            self.error(
                request.id.as_deref(),
                ErrorCode::BadRequest,
                "`lines` or `to` is required",
            );
            return;
        };
        sub.scroll_target = Some((target, now));
        if let Err(err) = self
            .call(
                "pane.scroll",
                json!({ "pane_id": pane_id, "offset_from_bottom": target }),
            )
            .await
        {
            self.api_error(request.id.as_deref(), err);
        }
    }

    async fn surface_history(&mut self, request: protocol::SurfaceHistory) {
        let Some(sub) = self.subs.get(&request.sub) else {
            self.error(
                Some(&request.id),
                ErrorCode::NotFound,
                "no such subscription",
            );
            return;
        };
        let pane_id = sub.pane_id.clone();
        let result = self
            .call(
                "pane.read",
                json!({ "pane_id": pane_id, "source": "recent", "format": "text", "lines": request.lines() }),
            )
            .await;
        match result {
            Ok(response) => {
                let text = api_proxy::result_field(&response, "read")
                    .and_then(|read| read.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let lines = text.lines().collect::<Vec<_>>();
                let message = json!({
                    "t": "surface.history.result",
                    "id": request.id,
                    "sub": request.sub,
                    "lines": lines,
                })
                .to_string();
                if message.len() > super::MAX_OUTBOUND_BYTES {
                    self.error(
                        Some(&request.id),
                        ErrorCode::PayloadTooLarge,
                        "history exceeds 4 MiB",
                    );
                } else {
                    self.send(message);
                }
            }
            Err(err) => self.api_error(Some(&request.id), err),
        }
    }

    async fn input(&mut self, input: protocol::Input) {
        let id = input.id.as_deref();
        let (pane_target, terminal_target) = match (input.sub, &input.target) {
            (Some(sub), _) => match self.subs.get(&sub) {
                Some(handle) => (handle.pane_id.clone(), handle.terminal_id.clone()),
                None => {
                    self.error(id, ErrorCode::NotFound, "no such subscription");
                    return;
                }
            },
            (None, Some(target)) => (target.clone(), target.clone()),
            (None, None) => {
                self.error(id, ErrorCode::BadRequest, "`sub` or `target` is required");
                return;
            }
        };
        let result = match input.kind {
            InputKind::Text(text) => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let sent = self
                    .gateway
                    .server_event_tx
                    .send(ServerEvent::MobilePaneInput {
                        target: terminal_target,
                        events: vec![ClientPaneInputEvent::TextCommit(text)],
                        reply: reply_tx,
                    })
                    .await;
                if sent.is_err() {
                    self.error(id, ErrorCode::ServerUnavailable, "server loop is gone");
                    return;
                }
                match reply_rx.await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(message)) => {
                        self.error(id, ErrorCode::NotFound, message);
                        return;
                    }
                    Err(_) => {
                        self.error(id, ErrorCode::ServerUnavailable, "server did not answer");
                        return;
                    }
                }
            }
            InputKind::Key(keys) => self
                .call(
                    "pane.send_keys",
                    json!({ "pane_id": pane_target, "keys": keys }),
                )
                .await
                .map(|_| ()),
            InputKind::Paste(text) => self
                .call(
                    "pane.send_text",
                    json!({ "pane_id": pane_target, "text": text }),
                )
                .await
                .map(|_| ()),
            InputKind::Submit(text) => self
                .call(
                    "pane.send_input",
                    json!({ "pane_id": pane_target, "text": text, "keys": ["Enter"] }),
                )
                .await
                .map(|_| ()),
        };
        match result {
            Ok(()) => {
                if let Some(id) = id {
                    self.send(protocol::reply("input.ok", id));
                }
            }
            Err(err) => self.api_error(id, err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::store::tests::temp_dir;
    use tokio_tungstenite::tungstenite::http;

    fn hello(json: &str) -> Hello {
        match ClientMessage::decode(json).unwrap() {
            ClientMessage::Hello(hello) => hello,
            other => panic!("not hello: {other:?}"),
        }
    }

    #[test]
    fn auth_rejects_bad_tokens_and_blocks_the_ip_after_five_failures() {
        let dir = temp_dir("conn-auth");
        let gateway = Gateway::for_test(dir.clone());
        let (code, _) = pairing::create_pairing(&dir, 600, store::now_unix()).unwrap();
        let ip: IpAddr = "100.64.0.9".parse().unwrap();
        let paired = authenticate(
            &gateway,
            &hello(&format!(
                r#"{{"t":"hello","versions":[2],"app":{{"platform":"ios"}},"pair":{{"code":"{code}","device_name":"Phone","sid":"{}"}}}}"#,
                gateway.server_id
            )),
            ip,
        )
        .unwrap_or_else(|_| panic!("pairing failed"));
        let token = paired.token.clone().unwrap();
        let device_id = paired.device.device_id.clone();

        let ok = authenticate(
            &gateway,
            &hello(&format!(
                r#"{{"t":"hello","versions":[2],"auth":{{"device_id":"{device_id}","token":"{token}"}}}}"#
            )),
            ip,
        )
        .unwrap_or_else(|_| panic!("auth failed"));
        assert!(ok.token.is_none());

        let bad = hello(&format!(
            r#"{{"t":"hello","versions":[2],"auth":{{"device_id":"{device_id}","token":"fmd1_wrong"}}}}"#
        ));
        for _ in 0..AUTH_FAILURES_PER_IP {
            assert!(!ip_blocked(&gateway, ip));
            let (close_code, error) = authenticate(&gateway, &bad, ip).err().unwrap();
            assert_eq!(close_code, close::UNAUTHORIZED);
            assert!(error.contains("unauthorized"));
        }
        assert!(ip_blocked(&gateway, ip));
        assert!(!ip_blocked(&gateway, "100.64.0.10".parse().unwrap()));

        let unknown = hello(&format!(
            r#"{{"t":"hello","versions":[2],"auth":{{"device_id":"d_unknown","token":"{token}"}}}}"#
        ));
        assert_eq!(
            authenticate(&gateway, &unknown, "10.0.0.1".parse().unwrap())
                .err()
                .unwrap()
                .0,
            close::UNAUTHORIZED
        );

        // Revoked devices fail auth with their old token.
        gateway
            .update_devices(|devices| devices.remove(&device_id))
            .unwrap();
        let revoked = hello(&format!(
            r#"{{"t":"hello","versions":[2],"auth":{{"device_id":"{device_id}","token":"{token}"}}}}"#
        ));
        assert_eq!(
            authenticate(&gateway, &revoked, "10.0.0.2".parse().unwrap())
                .err()
                .unwrap()
                .0,
            close::UNAUTHORIZED
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hello_needs_exactly_one_of_auth_or_pair_and_pairing_is_rate_limited() {
        let dir = temp_dir("conn-hello");
        let gateway = Gateway::for_test(dir.clone());
        let ip: IpAddr = "100.64.0.1".parse().unwrap();
        let neither = hello(r#"{"t":"hello","versions":[2]}"#);
        assert_eq!(
            authenticate(&gateway, &neither, ip).err().unwrap().0,
            close::PROTOCOL_ERROR
        );
        let both = hello(
            r#"{"t":"hello","versions":[2],"auth":{"device_id":"d","token":"t"},"pair":{"code":"c","sid":"s"}}"#,
        );
        assert_eq!(
            authenticate(&gateway, &both, ip).err().unwrap().0,
            close::PROTOCOL_ERROR
        );
        let pair = hello(r#"{"t":"hello","versions":[2],"pair":{"code":"c","sid":"s"}}"#);
        for attempt in 0..PAIR_ATTEMPTS {
            let addr = IpAddr::from([10, 0, 1, attempt as u8]);
            assert_eq!(
                authenticate(&gateway, &pair, addr).err().unwrap().0,
                close::PAIRING_FAILED
            );
        }
        assert_eq!(
            authenticate(&gateway, &pair, "10.0.2.1".parse().unwrap())
                .err()
                .unwrap()
                .0,
            close::RATE_LIMITED
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn welcome_includes_the_token_only_on_the_pair_path() {
        let dir = temp_dir("conn-welcome");
        let gateway = Gateway::for_test(dir.clone());
        let device = DeviceRecord {
            device_id: "d_q8Zt3mVfR1xKp0aB".into(),
            name: "Dylan's iPhone".into(),
            platform: "ios".into(),
            token_sha256: String::new(),
            created_at: 0,
            last_seen_at: 0,
            push: None,
            extra: Default::default(),
        };
        let paired = Authenticated {
            device: device.clone(),
            token: Some("fmd1_x".into()),
        };
        let value: Value = serde_json::from_str(&welcome(&gateway, &paired, 2)).unwrap();
        assert_eq!(value["t"], "welcome");
        assert_eq!(value["device"]["token"], "fmd1_x");
        assert_eq!(value["server"]["server_id"], json!(gateway.server_id));
        assert_eq!(value["limits"]["max_cols"], 500);
        let fixture: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/mobile-v2/server.welcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let keys = |value: &Value| {
            let mut keys = value
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            keys.sort();
            keys
        };
        assert_eq!(keys(&value), keys(&fixture));
        assert_eq!(keys(&value["limits"]), keys(&fixture["limits"]));
        let reconnect = Authenticated {
            device,
            token: None,
        };
        let value: Value = serde_json::from_str(&welcome(&gateway, &reconnect, 2)).unwrap();
        assert!(value["device"].get("token").is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn request(path: &str, protocol_header: Option<&str>) -> Request {
        let mut builder = http::Request::builder().uri(path);
        if let Some(value) = protocol_header {
            builder = builder.header("Sec-WebSocket-Protocol", value);
        }
        builder.body(()).unwrap()
    }

    #[test]
    fn upgrade_requires_path_and_subprotocol() {
        let ok = check_upgrade(
            &request("/mobile/v2", Some("chat, flockr.mobile.v2")),
            Response::new(()),
        )
        .unwrap();
        assert_eq!(
            ok.headers().get("Sec-WebSocket-Protocol").unwrap(),
            "flockr.mobile.v2"
        );
        for path in ["/other", "/mobile/v1"] {
            assert_eq!(
                check_upgrade(&request(path, Some("flockr.mobile.v2")), Response::new(()))
                    .unwrap_err()
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
        for header in [None, Some("flockr.mobile.v1")] {
            assert_eq!(
                check_upgrade(&request("/mobile/v2", header), Response::new(()))
                    .unwrap_err()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
    }

    mod wire {
        use super::*;
        use crate::server::mobile::noise::tests::{finish, initiator, write_msg1};
        use crate::server::mobile::noise::MAX_CHUNK_LEN;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        /// A gateway in a temp dir with `serve` behind a loopback listener.
        struct Server {
            gateway: Arc<Gateway>,
            addr: SocketAddr,
            dir: std::path::PathBuf,
        }

        impl Drop for Server {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        async fn server(name: &str) -> Server {
            let dir = temp_dir(name);
            let gateway = Gateway::for_test(dir.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let serving = gateway.clone();
            tokio::spawn(async move {
                while let Ok((stream, peer)) = listener.accept().await {
                    tokio::spawn(serve(serving.clone(), stream, peer));
                }
            });
            Server { gateway, addr, dir }
        }

        impl Server {
            fn public_key(&self) -> [u8; 32] {
                store::noise_public_key(&self.gateway.noise_key).unwrap()
            }

            fn failures(&self) -> usize {
                self.gateway
                    .state()
                    .ip_failures
                    .get_mut(&IpAddr::from([127, 0, 0, 1]))
                    .map_or(0, |window| {
                        window.count(Instant::now(), AUTH_FAILURE_WINDOW)
                    })
            }
        }

        async fn connect(addr: SocketAddr) -> Ws {
            let stream = TcpStream::connect(addr).await.unwrap();
            let mut request = format!("ws://{addr}/mobile/v2")
                .into_client_request()
                .unwrap();
            request.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                HeaderValue::from_static("flockr.mobile.v2"),
            );
            tokio_tungstenite::client_async(request, stream)
                .await
                .unwrap()
                .0
        }

        async fn handshake(ws: &mut Ws, server_public: &[u8]) -> Session {
            let mut client = initiator(server_public, None);
            ws.send(Message::Binary(write_msg1(&mut client).into()))
                .await
                .unwrap();
            match ws.next().await {
                Some(Ok(Message::Binary(msg2))) => finish(client, &msg2),
                other => panic!("expected handshake message 2, got {other:?}"),
            }
        }

        async fn send(ws: &mut Ws, session: &mut Session, text: &str) {
            for frame in session.seal(text.as_bytes()).unwrap() {
                ws.send(Message::Binary(frame.into())).await.unwrap();
            }
        }

        /// Next JSON message plus how many frames carried it.
        async fn recv(ws: &mut Ws, session: &mut Session) -> (Value, usize) {
            let mut frames = 0;
            loop {
                match ws.next().await {
                    Some(Ok(Message::Binary(frame))) => {
                        frames += 1;
                        if let Some(bytes) = session.open(&frame, usize::MAX).unwrap() {
                            return (serde_json::from_slice(&bytes).unwrap(), frames);
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    other => panic!("expected a binary frame, got {other:?}"),
                }
            }
        }

        async fn close_code(ws: &mut Ws) -> u16 {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Close(Some(frame)))) => return u16::from(frame.code),
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    other => panic!("expected a close frame, got {other:?}"),
                }
            }
        }

        #[tokio::test]
        async fn pairs_over_noise_and_chunks_large_messages_both_ways() {
            let server = server("wire-pair").await;
            let (code, _) = pairing::create_pairing(&server.dir, 600, store::now_unix()).unwrap();
            let mut ws = connect(server.addr).await;
            let mut session = handshake(&mut ws, &server.public_key()).await;
            let hello = json!({
                "t": "hello",
                "versions": [2],
                "app": { "platform": "ios" },
                "pair": { "code": code, "device_name": "Phone", "sid": server.gateway.server_id },
            });
            send(&mut ws, &mut session, &hello.to_string()).await;
            let (welcome, _) = recv(&mut ws, &mut session).await;
            assert_eq!(welcome["t"], "welcome");
            assert_eq!(welcome["version"], 2);
            assert!(welcome["device"]["token"]
                .as_str()
                .unwrap()
                .starts_with("fmd1_"));

            // Phone → server: a paste larger than one chunk is reassembled and
            // handled (the test gateway has no API socket, so it fails there).
            let paste = json!({
                "t": "input.paste",
                "id": "i1",
                "target": "p_1",
                "text": "x".repeat(MAX_CHUNK_LEN + 100),
            });
            send(&mut ws, &mut session, &paste.to_string()).await;
            let (reply, frames) = recv(&mut ws, &mut session).await;
            assert_eq!(frames, 1);
            assert_eq!(reply["t"], "error");
            assert_eq!(reply["id"], "i1");
            assert_eq!(reply["code"], "server_unavailable");

            // Server → phone: a message larger than one chunk.
            let big = json!({ "t": "test.big", "data": "y".repeat(MAX_CHUNK_LEN * 2) });
            let out = server
                .gateway
                .state()
                .conns
                .values()
                .next()
                .unwrap()
                .out
                .clone();
            assert!(out.send(big.to_string()));
            let (received, frames) = recv(&mut ws, &mut session).await;
            assert_eq!(frames, 3);
            assert_eq!(received, big);
        }

        #[tokio::test]
        async fn unsupported_version_is_rejected_inside_the_channel() {
            let server = server("wire-version").await;
            let mut ws = connect(server.addr).await;
            let mut session = handshake(&mut ws, &server.public_key()).await;
            send(&mut ws, &mut session, r#"{"t":"hello","versions":[1]}"#).await;
            let (error, _) = recv(&mut ws, &mut session).await;
            assert_eq!(error["code"], "unsupported_version");
            assert_eq!(error["supported"], json!([2]));
            assert_eq!(close_code(&mut ws).await, close::UNSUPPORTED_VERSION);
        }

        #[tokio::test]
        async fn wrong_server_key_closes_4006_and_counts_as_a_failure() {
            let server = server("wire-wrong-key").await;
            let other = store::noise_public_key(&[9_u8; 32]).unwrap();
            let mut ws = connect(server.addr).await;
            let mut client = initiator(&other, None);
            ws.send(Message::Binary(write_msg1(&mut client).into()))
                .await
                .unwrap();
            assert_eq!(close_code(&mut ws).await, close::HANDSHAKE_FAILED);
            assert_eq!(server.failures(), 1);

            // A short handshake message fails the same way.
            let mut ws = connect(server.addr).await;
            ws.send(Message::Binary(vec![0_u8; 32].into()))
                .await
                .unwrap();
            assert_eq!(close_code(&mut ws).await, close::HANDSHAKE_FAILED);
            assert_eq!(server.failures(), 2);
        }

        #[tokio::test]
        async fn text_frames_close_4000() {
            let server = server("wire-text").await;
            let mut ws = connect(server.addr).await;
            ws.send(Message::Text(r#"{"t":"hello"}"#.into()))
                .await
                .unwrap();
            assert_eq!(close_code(&mut ws).await, close::PROTOCOL_ERROR);

            let mut ws = connect(server.addr).await;
            handshake(&mut ws, &server.public_key()).await;
            ws.send(Message::Text(r#"{"t":"hello","versions":[2]}"#.into()))
                .await
                .unwrap();
            assert_eq!(close_code(&mut ws).await, close::PROTOCOL_ERROR);
        }

        #[tokio::test]
        async fn hello_over_the_cap_closes_1009_before_it_is_complete() {
            let server = server("wire-cap").await;
            let mut ws = connect(server.addr).await;
            let mut session = handshake(&mut ws, &server.public_key()).await;
            let frames = session.seal(&vec![b' '; MAX_CHUNK_LEN * 3]).unwrap();
            // Two "more chunks follow" frames pass the 64 KiB hello cap; the
            // last chunk is never sent.
            for frame in frames.into_iter().take(2) {
                assert_eq!(frame.len(), noise::MAX_FRAME_LEN);
                ws.send(Message::Binary(frame.into())).await.unwrap();
            }
            assert_eq!(close_code(&mut ws).await, close::TOO_BIG);
        }
    }
}
