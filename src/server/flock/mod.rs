//! The flock node: accepts connections from other members, dials the ones it
//! isn't connected to, keeps the membership log in sync and completes joins
//! approved by the owner.
//!
//! Membership is checked against the log on every connection; there are no
//! expiring credentials. A member that missed a removal learns it from the
//! first other member it syncs with, so the node catches up with every
//! reachable member after it starts or wakes before it reports itself caught
//! up, and every connection syncs the log both ways before it counts as
//! synced.
//!
//! The CLI writes files (log entries, invites, decisions) and this node
//! notices them on its poll tick, as the mobile gateway does for devices.

mod conn;

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};

use crate::config::FlockConfig;
use crate::flock::keys::MemberKeys;
use crate::flock::log::{Entry, Op, State};
use crate::flock::store;
use crate::mobile::store::now_unix;
use crate::server::mobile::RateWindow;

const POLL: Duration = Duration::from_secs(2);
const DIAL_INTERVAL: Duration = Duration::from_secs(5);
const DIAL_BACKOFF_MAX: Duration = Duration::from_secs(60);
const BIND_RETRY: Duration = Duration::from_secs(10);
const MAX_UNAUTHENTICATED: usize = 8;
const JOIN_FAILURES_PER_IP: usize = 5;
const JOIN_FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// Longest a catch-up waits for members that don't answer.
const CATCH_UP_LIMIT: Duration = Duration::from_secs(20);
/// Wall-clock time passing this much faster than monotonic time between polls
/// means the machine slept.
const SLEEP_GAP: Duration = Duration::from_secs(30);

/// What a connection is told when the log changes.
#[derive(Clone)]
pub(super) enum Broadcast {
    /// Entries stored since the last poll.
    Entries(Arc<Vec<Entry>>),
}

struct ConnEntry {
    member: String,
    /// Whether this node dialed the connection.
    outbound: bool,
    close: watch::Sender<bool>,
}

/// Members still to reach after a start or wake.
struct CatchUp {
    until: Instant,
    pending: HashSet<String>,
}

struct Shared {
    state: Option<State>,
    entries: Vec<Entry>,
    stamp: Option<SystemTime>,
    conns: HashMap<u64, ConnEntry>,
    /// Entry hashes each member is known to hold.
    peer_hashes: HashMap<String, HashSet<String>>,
    /// Unix time of the last completed two-way sync per member.
    last_sync: HashMap<String, u64>,
    catch_up: Option<CatchUp>,
    unauthenticated: usize,
    join_failures: HashMap<IpAddr, RateWindow>,
    next_dial: HashMap<String, (Instant, Duration)>,
    last_poll: Option<(Instant, SystemTime)>,
}

pub(crate) struct Node {
    dir: PathBuf,
    keys: MemberKeys,
    me: String,
    configured_addrs: Vec<String>,
    shared: Mutex<Shared>,
    events: broadcast::Sender<Broadcast>,
    /// Bumped when the member list changes, so connections recheck their peer.
    members_version: watch::Sender<u64>,
    shutdown: watch::Receiver<bool>,
    next_conn_id: AtomicU64,
    bound: Mutex<Vec<String>>,
}

impl Node {
    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn state(&self) -> Option<State> {
        self.shared().state.clone()
    }

    pub(super) fn entries(&self) -> Vec<Entry> {
        self.shared().entries.clone()
    }

    pub(super) fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub(super) fn me(&self) -> &str {
        &self.me
    }

    pub(super) fn keys(&self) -> &MemberKeys {
        &self.keys
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<Broadcast> {
        self.events.subscribe()
    }

    pub(super) fn members_version(&self) -> watch::Receiver<u64> {
        self.members_version.subscribe()
    }

    pub(super) fn shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown.clone()
    }

    /// Registers an authenticated connection. Two members dialing each other
    /// at once end up with two connections; both sides keep the one the
    /// lower member ID dialed. `None` means this one is the duplicate.
    pub(super) fn register(
        &self,
        member: &str,
        outbound: bool,
    ) -> Option<(u64, watch::Receiver<bool>)> {
        let preferred = |outbound: bool| {
            if outbound {
                self.me.as_str() < member
            } else {
                member < self.me.as_str()
            }
        };
        let mut shared = self.shared();
        let existing: Vec<u64> = shared
            .conns
            .iter()
            .filter(|(_, entry)| entry.member == member)
            .map(|(id, _)| *id)
            .collect();
        for id in existing {
            let keep_existing = shared
                .conns
                .get(&id)
                .is_some_and(|entry| preferred(entry.outbound) || !preferred(outbound));
            if keep_existing {
                return None;
            }
            if let Some(entry) = shared.conns.remove(&id) {
                let _ = entry.close.send(true);
            }
        }
        let id = self.next_conn_id.fetch_add(1, Ordering::Relaxed);
        let (close, closed) = watch::channel(false);
        shared.conns.insert(
            id,
            ConnEntry {
                member: member.to_owned(),
                outbound,
                close,
            },
        );
        Some((id, closed))
    }

    pub(super) fn unregister(&self, conn: u64) {
        self.shared().conns.remove(&conn);
    }

    fn connected(&self) -> HashSet<String> {
        self.shared()
            .conns
            .values()
            .map(|entry| entry.member.clone())
            .collect()
    }

    /// Records entry hashes `member` holds.
    pub(super) fn peer_has(&self, member: &str, hashes: impl IntoIterator<Item = String>) {
        self.shared()
            .peer_hashes
            .entry(member.to_owned())
            .or_default()
            .extend(hashes);
    }

    /// Replaces what `member` is known to hold with its own report.
    pub(super) fn peer_reported(&self, member: &str, hashes: HashSet<String>) {
        self.shared().peer_hashes.insert(member.to_owned(), hashes);
    }

    /// Both sides now hold each other's entries.
    pub(super) fn synced(&self, member: &str) {
        let mut shared = self.shared();
        shared.last_sync.insert(member.to_owned(), now_unix());
        if let Some(catch_up) = shared.catch_up.as_mut() {
            catch_up.pending.remove(member);
        }
    }

    /// A dial to `member` failed at every address.
    pub(super) fn unreachable(&self, member: &str) {
        if let Some(catch_up) = self.shared().catch_up.as_mut() {
            catch_up.pending.remove(member);
        }
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

    /// Stores entries from a peer: valid signatures and this group only.
    pub(super) fn store_entries(&self, entries: &[Entry]) -> usize {
        let Some(group) = self
            .shared()
            .state
            .as_ref()
            .map(|state| state.group.clone())
        else {
            return 0;
        };
        let mut stored = 0;
        for entry in entries {
            let Some(body) = entry.verified_body() else {
                continue;
            };
            if body.group != group {
                continue;
            }
            match store::save_entry(&self.dir, entry) {
                Ok(true) => stored += 1,
                Ok(false) => {}
                Err(err) => warn!(err = %err, "could not store a flock log entry"),
            }
        }
        if stored > 0 {
            self.reload();
        }
        stored
    }

    /// Re-reads the log: folds it, tells connections about new entries and
    /// member changes, and publishes this member's addresses.
    fn reload(&self) {
        let stamp = store::log_stamp(&self.dir);
        let state = match store::load_state(&self.dir) {
            Ok(state) => state,
            Err(err) => {
                warn!(err = %err, "could not read the flock log");
                return;
            }
        };
        let entries = store::load_entries(&self.dir).unwrap_or_default();
        let (new_entries, members_changed) = {
            let mut shared = self.shared();
            let known: HashSet<String> = shared.entries.iter().map(Entry::hash).collect();
            let new_entries: Vec<Entry> = entries
                .iter()
                .filter(|entry| !known.contains(&entry.hash()))
                .cloned()
                .collect();
            let before = shared.state.as_ref().map(|state| state.members.clone());
            let after = state.as_ref().map(|state| state.members.clone());
            shared.state = state;
            shared.entries = entries;
            shared.stamp = stamp;
            (new_entries, before != after)
        };
        if !new_entries.is_empty() {
            debug!(count = new_entries.len(), "flock log changed");
            let _ = self.events.send(Broadcast::Entries(Arc::new(new_entries)));
        }
        if members_changed {
            self.members_version.send_modify(|version| *version += 1);
        }
        self.publish_addrs();
    }

    /// Signs a `set_addrs` entry when the configured addresses differ from
    /// the log's.
    fn publish_addrs(&self) {
        let Some(state) = self.state() else {
            return;
        };
        let Some(me) = state.active(&self.me) else {
            return;
        };
        if me.info.addrs == self.configured_addrs {
            return;
        }
        let entry = state.append(
            Op::SetAddrs {
                member: self.me.clone(),
                addrs: self.configured_addrs.clone(),
            },
            &self.keys.sign,
            now_unix(),
        );
        match store::save_entry(&self.dir, &entry) {
            Ok(_) => info!(addrs = ?self.configured_addrs, "published flock addresses"),
            Err(err) => warn!(err = %err, "could not publish flock addresses"),
        }
    }

    /// Starts catching up with every member this node can dial: after a
    /// start, or after the machine slept and may have missed removals.
    fn start_catch_up(&self, reason: &str) {
        let mut shared = self.shared();
        let Some(state) = shared.state.clone() else {
            return;
        };
        let pending: HashSet<String> = state
            .members
            .iter()
            .filter(|(id, member)| {
                !member.removed && **id != self.me && !member.info.addrs.is_empty()
            })
            .map(|(id, _)| id.clone())
            .collect();
        info!(reason, members = pending.len(), "flock catching up");
        shared.next_dial.clear();
        shared.catch_up = Some(CatchUp {
            until: Instant::now() + CATCH_UP_LIMIT,
            pending,
        });
    }

    fn poll(&self) {
        let now = (Instant::now(), SystemTime::now());
        let slept = self
            .shared()
            .last_poll
            .replace(now)
            .is_some_and(|(mono, wall)| {
                let wall_gap = now.1.duration_since(wall).unwrap_or_default();
                wall_gap.saturating_sub(now.0.duration_since(mono)) > SLEEP_GAP
            });
        let stamp = store::log_stamp(&self.dir);
        let first_state = self.shared().state.is_none();
        if stamp != self.shared().stamp || first_state {
            self.reload();
            if first_state && self.shared().state.is_some() {
                self.start_catch_up("start");
            }
        }
        if slept {
            self.start_catch_up("wake");
        }
        let caught_up = {
            let mut shared = self.shared();
            let done = shared.catch_up.as_ref().is_some_and(|catch_up| {
                catch_up.pending.is_empty() || Instant::now() > catch_up.until
            });
            if done {
                if let Some(catch_up) = shared.catch_up.take() {
                    info!(unreached = catch_up.pending.len(), "flock caught up");
                }
            }
            shared.catch_up.is_none()
        };
        self.write_node_record(caught_up);
    }

    fn write_node_record(&self, caught_up: bool) {
        let (connected, behind, last_sync) = {
            let shared = self.shared();
            let mine: HashSet<String> = shared.entries.iter().map(Entry::hash).collect();
            let mut connected: Vec<String> = shared
                .conns
                .values()
                .map(|entry| entry.member.clone())
                .collect();
            connected.sort();
            connected.dedup();
            let mut behind: Vec<String> = shared
                .state
                .iter()
                .flat_map(|state| state.members.iter())
                .filter(|(id, member)| !member.removed && **id != self.me)
                .filter(|(id, _)| {
                    shared
                        .peer_hashes
                        .get(*id)
                        .is_none_or(|theirs| !mine.is_subset(theirs))
                })
                .map(|(id, _)| id.clone())
                .collect();
            behind.sort();
            (connected, behind, shared.last_sync.clone())
        };
        let record = store::NodeRecord {
            version: 2,
            pid: std::process::id(),
            bound: self
                .bound
                .lock()
                .map(|bound| bound.clone())
                .unwrap_or_default(),
            connected,
            behind,
            last_sync,
            caught_up,
            updated_at: now_unix(),
        };
        let changed = store::load_node(&self.dir).is_none_or(|old| !old.same_as(&record));
        if changed {
            if let Err(err) = store::save_node(&self.dir, &record) {
                warn!(err = %err, "could not write flock node.json");
            }
        }
    }

    /// Members to dial now: active, not this machine, with addresses, not
    /// connected, and past their backoff.
    fn due_dials(&self) -> Vec<(String, String, Vec<String>)> {
        let connected = self.connected();
        let mut shared = self.shared();
        let Some(state) = shared.state.clone() else {
            return Vec::new();
        };
        if state.active(&self.me).is_none() {
            return Vec::new();
        }
        let now = Instant::now();
        let mut due = Vec::new();
        for (id, member) in &state.members {
            if member.removed
                || *id == self.me
                || member.info.addrs.is_empty()
                || connected.contains(id)
            {
                continue;
            }
            if shared.next_dial.get(id).is_some_and(|(at, _)| *at > now) {
                continue;
            }
            due.push((
                id.clone(),
                member.info.noise_key.clone(),
                member.info.addrs.clone(),
            ));
        }
        for (id, _, _) in &due {
            let backoff = shared
                .next_dial
                .get(id)
                .map(|(_, backoff)| (*backoff * 2).min(DIAL_BACKOFF_MAX))
                .unwrap_or(DIAL_INTERVAL);
            shared
                .next_dial
                .insert(id.clone(), (now + backoff, backoff));
        }
        due
    }

    pub(super) fn dial_succeeded(&self, member: &str) {
        self.shared().next_dial.remove(member);
    }
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
            warn!(err = %err, "flock could not load this member's keys; not starting");
            return None;
        }
    };
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (events, _) = broadcast::channel(64);
    let (members_version, _) = watch::channel(0);
    let node = Arc::new(Node {
        dir: dir.clone(),
        me: keys.member_id(),
        keys,
        configured_addrs: addrs.iter().map(SocketAddr::to_string).collect(),
        shared: Mutex::new(Shared {
            state: None,
            entries: Vec::new(),
            stamp: None,
            conns: HashMap::new(),
            peer_hashes: HashMap::new(),
            last_sync: HashMap::new(),
            catch_up: None,
            unauthenticated: 0,
            join_failures: HashMap::new(),
            next_dial: HashMap::new(),
            last_poll: None,
        }),
        events,
        members_version,
        shutdown: shutdown_rx,
        next_conn_id: AtomicU64::new(1),
        bound: Mutex::new(Vec::new()),
    });
    node.poll();
    for addr in addrs {
        tokio::spawn(listen(node.clone(), addr));
    }
    tokio::spawn(poll_loop(node.clone()));
    tokio::spawn(dial_loop(node.clone()));
    info!(member = %node.me, "flock node started");
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

async fn dial_loop(node: Arc<Node>) {
    let mut shutdown = node.shutdown();
    let mut tick = tokio::time::interval(DIAL_INTERVAL);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                for (member, noise_key, addrs) in node.due_dials() {
                    tokio::spawn(conn::dial(node.clone(), member, noise_key, addrs));
                }
            }
            _ = shutdown.changed() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(me: &str) -> Node {
        let (_, shutdown) = watch::channel(false);
        Node {
            dir: PathBuf::new(),
            keys: MemberKeys::generate().unwrap(),
            me: me.to_owned(),
            configured_addrs: Vec::new(),
            shared: Mutex::new(Shared {
                state: None,
                entries: Vec::new(),
                stamp: None,
                conns: HashMap::new(),
                peer_hashes: HashMap::new(),
                last_sync: HashMap::new(),
                catch_up: None,
                unauthenticated: 0,
                join_failures: HashMap::new(),
                next_dial: HashMap::new(),
                last_poll: None,
            }),
            events: broadcast::channel(4).0,
            members_version: watch::channel(0).0,
            shutdown,
            next_conn_id: AtomicU64::new(1),
            bound: Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn both_sides_keep_the_connection_the_lower_id_dialed() {
        // "m_a" < "m_b": the connection m_a dialed wins on both machines.
        let a = node("m_a");
        let (_, a_out_closed) = a.register("m_b", true).unwrap();
        assert!(a.register("m_b", false).is_none());
        assert!(!*a_out_closed.borrow());

        let b = node("m_b");
        let (_, b_out_closed) = b.register("m_a", true).unwrap();
        // The preferred inbound connection arrives later and replaces it.
        assert!(b.register("m_a", false).is_some());
        assert!(*b_out_closed.borrow());
        assert_eq!(b.connected().len(), 1);
    }
}
