//! The flock node: accepts connections from other members, dials the ones it
//! isn't connected to, keeps the membership log in sync, renews certificates
//! and completes joins approved by the owner.
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
use crate::flock::cert::{self, Cert};
use crate::flock::keys::{encode_key, MemberKeys};
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

/// What a connection is told when the log changes.
#[derive(Clone)]
pub(super) enum Broadcast {
    /// Entries stored since the last poll.
    Entries(Arc<Vec<Entry>>),
    /// This member's certificate was renewed.
    OwnCert(Cert),
}

struct Shared {
    state: Option<State>,
    entries: Vec<Entry>,
    stamp: Option<SystemTime>,
    /// conn id -> peer member ID, for authenticated connections.
    conns: HashMap<u64, String>,
    unauthenticated: usize,
    join_failures: HashMap<IpAddr, RateWindow>,
    next_dial: HashMap<String, (Instant, Duration)>,
}

pub(crate) struct Node {
    dir: PathBuf,
    keys: MemberKeys,
    me: String,
    noise_public: String,
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

    pub(super) fn next_conn_id(&self) -> u64 {
        self.next_conn_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) fn register(&self, conn: u64, member: &str) {
        self.shared().conns.insert(conn, member.to_owned());
    }

    pub(super) fn unregister(&self, conn: u64) {
        self.shared().conns.remove(&conn);
    }

    fn connected(&self) -> HashSet<String> {
        self.shared().conns.values().cloned().collect()
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

    /// Stores entries from a peer. Valid signatures and this group only; the
    /// poll tick folds them in and passes them on.
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

    /// Saves a certificate a peer issued for this member, if it checks out
    /// and lasts longer than the current one.
    pub(super) fn accept_own_cert(&self, cert: &Cert) {
        let Some(state) = self.state() else {
            return;
        };
        if cert.verify(&state, &self.noise_public, now_unix()).is_err() {
            return;
        }
        let current = store::load_cert(&self.dir).and_then(|cert| cert.parsed());
        let offered = cert.parsed();
        if offered.as_ref().map(|body| body.expires) > current.map(|body| body.expires) {
            self.save_own_cert(cert);
        }
    }

    /// A certificate for `member` if this node may issue one.
    pub(super) fn issue_cert(&self, member: &str) -> Option<Cert> {
        let state = self.state()?;
        let my_key = encode_key(&self.keys.sign_public());
        if !state.is_admin_key(&my_key) {
            return None;
        }
        cert::issue(&state, member, &self.keys.sign, now_unix())
    }

    /// Re-reads the log when it changed: folds it, tells connections about
    /// new entries and member changes, and publishes this member's addresses.
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
        self.renew_own_cert();
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

    fn renew_own_cert(&self) {
        let now = now_unix();
        if store::load_cert(&self.dir).is_some_and(|cert| !cert.needs_renewal(now)) {
            return;
        }
        if let Some(cert) = self.issue_cert(&self.me) {
            self.save_own_cert(&cert);
        }
    }

    fn save_own_cert(&self, cert: &Cert) {
        match store::save_cert(&self.dir, cert) {
            Ok(()) => {
                let _ = self.events.send(Broadcast::OwnCert(cert.clone()));
            }
            Err(err) => warn!(err = %err, "could not save this member's flock certificate"),
        }
    }

    fn poll(&self) {
        let stamp = store::log_stamp(&self.dir);
        if stamp != self.shared().stamp || self.shared().state.is_none() {
            self.reload();
        } else {
            self.renew_own_cert();
        }
        let record = store::NodeRecord {
            version: 1,
            pid: std::process::id(),
            bound: self
                .bound
                .lock()
                .map(|bound| bound.clone())
                .unwrap_or_default(),
            connected: {
                let mut connected: Vec<String> = self.connected().into_iter().collect();
                connected.sort();
                connected
            },
            updated_at: now_unix(),
        };
        let changed = store::load_node(&self.dir).is_none_or(|old| {
            old.pid != record.pid || old.bound != record.bound || old.connected != record.connected
        });
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
    let noise_public = match keys.noise_public() {
        Ok(key) => encode_key(&key),
        Err(err) => {
            warn!(err = %err, "flock could not derive its noise key; not starting");
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
        noise_public,
        configured_addrs: addrs.iter().map(SocketAddr::to_string).collect(),
        shared: Mutex::new(Shared {
            state: None,
            entries: Vec::new(),
            stamp: None,
            conns: HashMap::new(),
            unauthenticated: 0,
            join_failures: HashMap::new(),
            next_dial: HashMap::new(),
        }),
        events,
        members_version,
        shutdown: shutdown_rx,
        next_conn_id: AtomicU64::new(1),
        bound: Mutex::new(Vec::new()),
    });
    node.reload();
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
