//! The flock node. On the hub it accepts connections from members, checks
//! each against `hub.json`, completes approved joins and sends every member
//! the member list. On other members it keeps one connection open to the
//! hub. Members never connect to each other.
//!
//! The CLI writes files (`hub.json`, invites, decisions) and this node
//! notices them on its poll tick, as the mobile gateway does for devices.

mod conn;
mod messages;
mod serve;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use crate::config::FlockConfig;
use crate::flock::keys::MemberKeys;
use crate::flock::store::{self, GroupFile, HubFile, Member};
use crate::mobile::store::now_unix;
use crate::server::mobile::RateWindow;

const POLL: Duration = Duration::from_secs(2);
const DIAL_INTERVAL: Duration = Duration::from_secs(5);
const DIAL_BACKOFF_MAX: Duration = Duration::from_secs(60);
const BIND_RETRY: Duration = Duration::from_secs(10);
const MAX_UNAUTHENTICATED: usize = 8;
const JOIN_FAILURES_PER_IP: usize = 5;
const JOIN_FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// What this machine is in the flock, from its files.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Membership {
    None,
    Hub(HubFile),
    Member(GroupFile),
}

struct ConnEntry {
    member: String,
    /// A CLI's connection rather than the member server's link.
    client: bool,
    tx: mpsc::UnboundedSender<Value>,
    close: watch::Sender<bool>,
}

/// A stream the hub relays from a client to a member's link.
#[derive(Clone)]
pub(super) struct Route {
    pub client_tx: mpsc::UnboundedSender<Value>,
    pub client_stream: u64,
    pub link_conn: u64,
}

/// A member's connection to the hub.
#[derive(Default)]
struct HubLink {
    connected: bool,
    dialing: bool,
    refused: Option<String>,
    next_dial: Option<Instant>,
    backoff: Option<Duration>,
}

struct Shared {
    membership: Membership,
    stamps: (Option<SystemTime>, Option<SystemTime>),
    /// Authenticated member connections, on the hub.
    conns: HashMap<u64, ConnEntry>,
    /// Relayed streams by the hub's stream ID.
    routes: HashMap<u64, Route>,
    unauthenticated: usize,
    join_failures: HashMap<IpAddr, RateWindow>,
    link: HubLink,
}

pub(crate) struct Node {
    dir: PathBuf,
    keys: MemberKeys,
    messages: Arc<messages::Messages>,
    shared: Mutex<Shared>,
    /// Bumped when the member list or the connected set changes, so hub
    /// connections recheck their peer and resend the list.
    roster: watch::Sender<u64>,
    shutdown: watch::Receiver<bool>,
    next_conn_id: AtomicU64,
    bound: Mutex<Vec<String>>,
}

fn file_stamp(path: PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

impl Node {
    fn new(dir: PathBuf, keys: MemberKeys, shutdown: watch::Receiver<bool>) -> Self {
        Self {
            messages: messages::Messages::new(dir.clone()),
            dir,
            keys,
            shared: Mutex::new(Shared {
                membership: Membership::None,
                stamps: (None, None),
                conns: HashMap::new(),
                routes: HashMap::new(),
                unauthenticated: 0,
                join_failures: HashMap::new(),
                link: HubLink::default(),
            }),
            roster: watch::channel(0).0,
            shutdown,
            next_conn_id: AtomicU64::new(1),
            bound: Mutex::new(Vec::new()),
        }
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn hub(&self) -> Option<HubFile> {
        match &self.shared().membership {
            Membership::Hub(hub) => Some(hub.clone()),
            _ => None,
        }
    }

    pub(super) fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub(super) fn keys(&self) -> &MemberKeys {
        &self.keys
    }

    pub(super) fn messages(&self) -> &Arc<messages::Messages> {
        &self.messages
    }

    pub(super) fn roster(&self) -> watch::Receiver<u64> {
        self.roster.subscribe()
    }

    pub(super) fn shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown.clone()
    }

    fn bump_roster(&self) {
        self.roster.send_modify(|version| *version += 1);
    }

    /// Registers an authenticated connection on the hub. A newer link from
    /// the same member replaces the older one, which may be half-open after a
    /// network change. Client connections never replace anything.
    pub(super) fn register(
        &self,
        member: &str,
        client: bool,
        tx: mpsc::UnboundedSender<Value>,
    ) -> (u64, watch::Receiver<bool>) {
        let id = self.next_id();
        let (close, closed) = watch::channel(false);
        {
            let mut shared = self.shared();
            if !client {
                shared.conns.retain(|_, entry| {
                    if !entry.client && entry.member == member {
                        let _ = entry.close.send(true);
                        false
                    } else {
                        true
                    }
                });
            }
            shared.conns.insert(
                id,
                ConnEntry {
                    member: member.to_owned(),
                    client,
                    tx,
                    close,
                },
            );
        }
        if !client {
            self.bump_roster();
        }
        (id, closed)
    }

    fn next_id(&self) -> u64 {
        self.next_conn_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Removes a connection. Streams relayed to a member's link close with a
    /// reason for their clients.
    pub(super) fn unregister(&self, conn: u64) {
        let (removed, orphaned) = {
            let mut shared = self.shared();
            let removed = shared.conns.remove(&conn);
            let orphaned: Vec<u64> = shared
                .routes
                .iter()
                .filter(|(_, route)| route.link_conn == conn)
                .map(|(id, _)| *id)
                .collect();
            let orphaned: Vec<Route> = orphaned
                .iter()
                .filter_map(|id| shared.routes.remove(id))
                .collect();
            (removed, orphaned)
        };
        for route in orphaned {
            let _ = route.client_tx.send(serde_json::json!({
                "t": "close",
                "s": route.client_stream,
                "error": "the member disconnected from the hub",
            }));
        }
        if removed.is_some_and(|entry| !entry.client) {
            self.bump_roster();
        }
    }

    /// The live link of `member`, if it is connected.
    pub(super) fn link(&self, member: &str) -> Option<(u64, mpsc::UnboundedSender<Value>)> {
        self.shared()
            .conns
            .iter()
            .find(|(_, entry)| !entry.client && entry.member == member)
            .map(|(id, entry)| (*id, entry.tx.clone()))
    }

    pub(super) fn add_route(&self, route: Route) -> u64 {
        let id = self.next_id();
        self.shared().routes.insert(id, route);
        id
    }

    pub(super) fn route(&self, id: u64) -> Option<Route> {
        self.shared().routes.get(&id).cloned()
    }

    pub(super) fn remove_route(&self, id: u64) -> Option<Route> {
        self.shared().routes.remove(&id)
    }

    /// Member IDs connected to the hub, the hub included.
    pub(super) fn connected(&self) -> Vec<String> {
        let shared = self.shared();
        let Membership::Hub(hub) = &shared.membership else {
            return Vec::new();
        };
        let mut connected: Vec<String> = shared
            .conns
            .values()
            .filter(|entry| !entry.client)
            .map(|entry| entry.member.clone())
            .chain([hub.hub.clone()])
            .collect();
        connected.sort();
        connected.dedup();
        connected
    }

    /// Takes an unauthenticated slot; `false` when all are in use.
    pub(super) fn try_unauthenticated(&self) -> bool {
        let mut shared = self.shared();
        if shared.unauthenticated >= MAX_UNAUTHENTICATED {
            return false;
        }
        shared.unauthenticated += 1;
        true
    }

    pub(super) fn release_unauthenticated(&self) {
        let mut shared = self.shared();
        shared.unauthenticated = shared.unauthenticated.saturating_sub(1);
    }

    pub(super) fn join_blocked(&self, ip: IpAddr) -> bool {
        self.shared()
            .join_failures
            .get_mut(&ip)
            .is_some_and(|window| {
                window.count(Instant::now(), JOIN_FAILURE_WINDOW) >= JOIN_FAILURES_PER_IP
            })
    }

    pub(super) fn record_join_failure(&self, ip: IpAddr) {
        self.shared()
            .join_failures
            .entry(ip)
            .or_default()
            .hit(Instant::now(), JOIN_FAILURE_WINDOW);
    }

    /// Re-reads this machine's flock files when they changed.
    pub(super) fn reload(&self) {
        let stamps = (
            file_stamp(self.dir.join(store::HUB_FILE)),
            file_stamp(self.dir.join(store::GROUP_FILE)),
        );
        if self.shared().stamps == stamps && stamps != (None, None) {
            return;
        }
        let membership = match (store::load_hub(&self.dir), store::load_group(&self.dir)) {
            (Ok(Some(hub)), _) => Membership::Hub(hub),
            (Ok(None), Ok(Some(group))) => Membership::Member(group),
            (Ok(None), Ok(None)) => Membership::None,
            (Err(err), _) | (_, Err(err)) => {
                warn!(err = %err, "could not read the flock files");
                return;
            }
        };
        let changed = {
            let mut shared = self.shared();
            shared.stamps = stamps;
            let changed = shared.membership != membership;
            if changed {
                if matches!(membership, Membership::Member(_))
                    && !matches!(shared.membership, Membership::Member(_))
                {
                    shared.link = HubLink::default();
                }
                shared.membership = membership;
            }
            changed
        };
        if changed {
            self.bump_roster();
        }
    }

    /// Whether the member should dial the hub now; marks the dial started.
    fn take_dial(&self) -> Option<GroupFile> {
        let mut shared = self.shared();
        let Membership::Member(group) = &shared.membership else {
            return None;
        };
        let group = group.clone();
        let link = &mut shared.link;
        if link.connected || link.dialing || link.next_dial.is_some_and(|at| at > Instant::now()) {
            return None;
        }
        link.dialing = true;
        Some(group)
    }

    /// A dial to the hub failed. `refused` is the hub's reason when it
    /// turned this member away; then the member waits the longest backoff.
    pub(super) fn dial_failed(&self, refused: Option<String>) {
        let mut shared = self.shared();
        let link = &mut shared.link;
        link.dialing = false;
        link.connected = false;
        let backoff = if refused.is_some() {
            DIAL_BACKOFF_MAX
        } else {
            link.backoff
                .map(|backoff| (backoff * 2).min(DIAL_BACKOFF_MAX))
                .unwrap_or(DIAL_INTERVAL)
        };
        if refused.is_some() {
            link.refused = refused;
        }
        link.backoff = Some(backoff);
        link.next_dial = Some(Instant::now() + backoff);
    }

    pub(super) fn hub_connected(&self) {
        let mut shared = self.shared();
        let link = &mut shared.link;
        link.connected = true;
        link.refused = None;
        link.backoff = None;
        link.next_dial = None;
    }

    /// A connection that was up ended. It retries soon, unless the hub
    /// refused this member.
    pub(super) fn hub_disconnected(&self, refused: Option<String>) {
        if refused.is_some() {
            return self.dial_failed(refused);
        }
        let mut shared = self.shared();
        let link = &mut shared.link;
        link.connected = false;
        link.dialing = false;
        link.backoff = None;
        link.next_dial = Some(Instant::now() + DIAL_INTERVAL);
    }

    fn poll(&self) {
        self.reload();
        self.write_node_record();
    }

    fn write_node_record(&self) {
        let (connected, refused) = {
            let shared = self.shared();
            match &shared.membership {
                Membership::Hub(_) => (None, None),
                Membership::Member(group) => (
                    Some(if shared.link.connected {
                        vec![group.hub.clone()]
                    } else {
                        Vec::new()
                    }),
                    shared.link.refused.clone(),
                ),
                Membership::None => (Some(Vec::new()), None),
            }
        };
        let record = store::NodeRecord {
            version: 3,
            pid: std::process::id(),
            bound: self
                .bound
                .lock()
                .map(|bound| bound.clone())
                .unwrap_or_default(),
            connected: connected.unwrap_or_else(|| self.connected()),
            refused,
            updated_at: now_unix(),
        };
        let changed = store::load_node(&self.dir).is_none_or(|old| !old.same_as(&record));
        if changed {
            if let Err(err) = store::save_node(&self.dir, &record) {
                warn!(err = %err, "could not write flock node.json");
            }
        }
    }
}

/// The member list a hub sends to members.
pub(super) fn roster_message(hub: &HubFile, connected: &[String]) -> serde_json::Value {
    let members: Vec<&Member> = hub.members.iter().collect();
    serde_json::json!({ "t": "members", "members": members, "connected": connected })
}

pub(crate) struct FlockNodeHandle {
    shutdown: watch::Sender<bool>,
    dir: PathBuf,
}

impl Drop for FlockNodeHandle {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        store::remove_node_if_owned(&self.dir, std::process::id());
    }
}

/// Starts the node when `[flock]` enables it. Must run inside the tokio
/// runtime.
pub(crate) fn start(config: &FlockConfig) -> Option<FlockNodeHandle> {
    if !config.enabled {
        return None;
    }
    let (addrs, diagnostics) = config.listen_addrs();
    for diagnostic in &diagnostics {
        warn!("{diagnostic}");
    }
    let dir = store::flock_dir();
    let keys = match crate::flock::keys::load_or_create(&dir) {
        Ok(keys) => keys,
        Err(err) => {
            warn!(err = %err, "flock could not load this machine's key; not starting");
            return None;
        }
    };
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let node = Arc::new(Node::new(dir.clone(), keys, shutdown_rx));
    node.poll();
    for addr in addrs {
        tokio::spawn(listen(node.clone(), addr));
    }
    tokio::spawn(poll_loop(node.clone()));
    tokio::spawn(dial_loop(node.clone()));
    tokio::spawn(delivery_loop(node.clone()));
    messages::serve_local(node.messages.clone(), node.shutdown());
    info!("flock node started");
    Some(FlockNodeHandle {
        shutdown: shutdown_tx,
        dir,
    })
}

async fn listen(node: Arc<Node>, addr: SocketAddr) {
    let mut shutdown = node.shutdown();
    let listener = loop {
        match TcpListener::bind(addr).await {
            Ok(listener) => break listener,
            Err(err) => {
                warn!(%addr, err = %err, "flock could not bind; retrying in 10 s");
                tokio::select! {
                    _ = tokio::time::sleep(BIND_RETRY) => {}
                    _ = shutdown.changed() => return,
                }
            }
        }
    };
    let bound = listener.local_addr().unwrap_or(addr);
    info!(addr = %bound, "flock listening");
    if crate::mobile::is_public_address(bound.ip()) {
        warn!("flock listening on public address {bound}; prefer a private network address");
    }
    if let Ok(mut list) = node.bound.lock() {
        list.push(bound.to_string());
    }
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    tokio::spawn(conn::serve_inbound(node.clone(), stream, peer));
                }
                Err(err) => {
                    warn!(err = %err, "flock accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            _ = shutdown.changed() => return,
        }
    }
}

async fn poll_loop(node: Arc<Node>) {
    let mut shutdown = node.shutdown();
    let mut tick = tokio::time::interval(POLL);
    loop {
        tokio::select! {
            _ = tick.tick() => node.poll(),
            _ = shutdown.changed() => return,
        }
    }
}

/// Tells idle agents about their unread messages.
async fn delivery_loop(node: Arc<Node>) {
    let mut shutdown = node.shutdown();
    let mut tick = tokio::time::interval(POLL);
    loop {
        tokio::select! {
            _ = tick.tick() => node.messages.deliver_due().await,
            _ = shutdown.changed() => return,
        }
    }
}

async fn dial_loop(node: Arc<Node>) {
    let mut shutdown = node.shutdown();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Some(group) = node.take_dial() {
                    tokio::spawn(conn::dial_hub(node.clone(), group));
                }
            }
            _ = shutdown.changed() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flock::store::tests::member;

    fn hub_node() -> Node {
        let node = Node::new(
            PathBuf::new(),
            MemberKeys::generate().unwrap(),
            watch::channel(false).1,
        );
        node.shared().membership = Membership::Hub(HubFile {
            version: 1,
            group: "g_1".into(),
            name: "home".into(),
            hub: "m_hub".into(),
            members: vec![member("m_hub", "k_hub"), member("m_a", "k_a")],
        });
        node
    }

    #[test]
    fn a_new_connection_from_a_member_replaces_its_old_one() {
        let node = hub_node();
        let tx = || mpsc::unbounded_channel().0;
        let (first, first_closed) = node.register("m_a", false, tx());
        let (_, client_closed) = node.register("m_a", true, tx());
        let (second, second_closed) = node.register("m_a", false, tx());
        assert!(*first_closed.borrow());
        assert!(!*second_closed.borrow());
        assert!(
            !*client_closed.borrow(),
            "a client doesn't replace the link"
        );
        assert_eq!(node.connected(), vec!["m_a".to_owned(), "m_hub".to_owned()]);
        assert_eq!(node.link("m_a").map(|(id, _)| id), Some(second));
        // The replaced connection's cleanup leaves the new one in place.
        node.unregister(first);
        assert_eq!(node.connected().len(), 2);
    }

    #[test]
    fn streams_relayed_to_a_member_close_when_its_link_drops() {
        let node = hub_node();
        let (link, _) = node.register("m_a", false, mpsc::unbounded_channel().0);
        let (client_tx, mut client_rx) = mpsc::unbounded_channel();
        let route = node.add_route(Route {
            client_tx,
            client_stream: 7,
            link_conn: link,
        });
        node.unregister(link);
        assert!(node.route(route).is_none());
        let close = client_rx.try_recv().unwrap();
        assert_eq!(close["t"], "close");
        assert_eq!(close["s"], 7);
        assert!(node.link("m_a").is_none());
    }

    #[test]
    fn a_refused_member_waits_the_longest_backoff() {
        let node = Node::new(
            PathBuf::new(),
            MemberKeys::generate().unwrap(),
            watch::channel(false).1,
        );
        node.shared().membership = Membership::Member(GroupFile {
            version: 1,
            group: "g_1".into(),
            name: "home".into(),
            member: "m_a".into(),
            hub: "m_hub".into(),
            hub_key: "k".into(),
            hub_addrs: Vec::new(),
        });
        assert!(node.take_dial().is_some());
        assert!(node.take_dial().is_none(), "one dial at a time");
        node.dial_failed(None);
        assert_eq!(node.shared().link.backoff, Some(DIAL_INTERVAL));
        node.shared().link.next_dial = None;
        assert!(node.take_dial().is_some());
        node.dial_failed(Some("removed from the flock".into()));
        let shared = node.shared();
        assert_eq!(shared.link.backoff, Some(DIAL_BACKOFF_MAX));
        assert_eq!(
            shared.link.refused.as_deref(),
            Some("removed from the flock")
        );
    }
}
