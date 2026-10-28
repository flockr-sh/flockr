//! One flock connection: handshake, member check or join, then log sync until
//! either side closes or the peer is removed.
//!
//! The Noise handshake proves the peer holds its static key; the log says
//! whether that key belongs to an active member. Right after the hello, both
//! sides send `log.have` and answer the other's with `log.entries` (marked
//! `reply`, possibly empty). Receiving the reply means this side holds
//! everything the peer had, so a removal the peer knows about has arrived.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info, warn};

use super::{Broadcast, Node};
use crate::flock::channel::{self, Channel};
use crate::flock::invite::{self, Decision, JoinRequest};
use crate::flock::keys::{self, decode_key, encode_key};
use crate::flock::log::{self, Entry, Kind, MemberInfo};
use crate::mobile::store::now_unix;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const DECISION_POLL: Duration = Duration::from_millis(500);
const PING_INTERVAL: Duration = Duration::from_secs(20);
const PEER_TIMEOUT: Duration = Duration::from_secs(60);
/// Sent instead of an error when a connection loses the duplicate check.
const DUPLICATE: &str = "duplicate";

fn error(reason: &str) -> Value {
    json!({ "t": "error", "reason": reason })
}

async fn refuse(mut channel: Channel, reason: &str) {
    debug!(reason, "flock connection refused");
    let _ = channel.send(&error(reason)).await;
    channel.close().await;
}

/// The active member that owns the peer's handshake key.
fn member_for(node: &Node, remote_static: &[u8; 32]) -> Result<String, &'static str> {
    let state = node.state().ok_or("this machine is not in a flock")?;
    state
        .active_by_noise_key(&encode_key(remote_static))
        .map(|member| member.info.id.clone())
        .ok_or("not a member of this flock")
}

pub(super) async fn serve_inbound(node: Arc<Node>, stream: TcpStream, peer: SocketAddr) {
    if !node.try_unauthenticated() {
        debug!(%peer, "flock connection dropped: too many waiting");
        return;
    }
    let outcome = accept(&node, stream, peer.ip()).await;
    node.release_unauthenticated();
    if let Some((channel, member)) = outcome {
        run(node, channel, member, false).await;
    }
}

async fn accept(node: &Arc<Node>, stream: TcpStream, ip: IpAddr) -> Option<(Channel, String)> {
    let group = node.state()?.group;
    let mut channel = match Channel::accept(stream, &node.keys().noise, &group).await {
        Ok(channel) => channel,
        Err(err) => {
            debug!(%ip, err = %err, "flock handshake failed");
            return None;
        }
    };
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, channel.recv()).await {
        Ok(Ok(Some(hello))) => hello,
        _ => return None,
    };
    match hello["t"].as_str() {
        Some("join") => {
            join(node, channel, &hello, ip).await;
            None
        }
        Some("hello") => {
            let remote = channel.remote_static;
            let member = match member_for(node, &remote) {
                Ok(member) if hello["member"] == member.as_str() => member,
                Ok(_) => {
                    refuse(channel, "member ID does not match the key").await;
                    return None;
                }
                Err(reason) => {
                    refuse(channel, reason).await;
                    return None;
                }
            };
            let welcome = json!({ "t": "welcome", "member": node.me() });
            channel.send(&welcome).await.ok()?;
            Some((channel, member))
        }
        _ => {
            refuse(channel, "expected hello").await;
            None
        }
    }
}

/// Connects to `member` at the first address that answers.
pub(super) async fn dial(node: Arc<Node>, member: String, noise_key: String, addrs: Vec<String>) {
    let (Some(state), Some(remote)) = (node.state(), decode_key(&noise_key)) else {
        return;
    };
    let mut channel = None;
    for addr in &addrs {
        match Channel::connect(addr, &node.keys().noise, &remote, &state.group).await {
            Ok(connected) => {
                channel = Some(connected);
                break;
            }
            Err(err) => debug!(%member, %addr, err = %err, "flock dial failed"),
        }
    }
    let Some(mut channel) = channel else {
        node.unreachable(&member);
        return;
    };
    let hello = json!({ "t": "hello", "member": node.me() });
    if channel.send(&hello).await.is_err() {
        node.unreachable(&member);
        return;
    }
    let welcome = match tokio::time::timeout(HELLO_TIMEOUT, channel.recv()).await {
        Ok(Ok(Some(welcome))) => welcome,
        _ => {
            node.unreachable(&member);
            return;
        }
    };
    if welcome["t"] != "welcome" || welcome["member"] != member.as_str() {
        warn!(%member, reason = %welcome["reason"], "flock peer refused the connection");
        node.unreachable(&member);
        channel.close().await;
        return;
    }
    node.dial_succeeded(&member);
    run(node, channel, member, true).await;
}

fn hashes(entries: &[Entry]) -> Vec<String> {
    entries.iter().map(Entry::hash).collect()
}

/// Authenticated connection loop.
async fn run(node: Arc<Node>, mut channel: Channel, member: String, outbound: bool) {
    let Some((conn, mut closed)) = node.register(&member, outbound) else {
        debug!(%member, "flock duplicate connection dropped");
        let _ = channel.send(&error(DUPLICATE)).await;
        channel.close().await;
        return;
    };
    info!(%member, "flock peer connected");
    let mut events = node.subscribe();
    let mut members = node.members_version();
    let mut shutdown = node.shutdown();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    let mut last_inbound = Instant::now();
    let have = hashes(&node.entries());
    let mut open = channel
        .send(&json!({ "t": "log.have", "hashes": have }))
        .await
        .is_ok();
    while open {
        tokio::select! {
            message = channel.recv() => match message {
                Ok(Some(message)) => {
                    last_inbound = Instant::now();
                    open = handle(&node, &mut channel, &member, &message).await;
                }
                Ok(None) => open = false,
                Err(err) => {
                    debug!(%member, err = %err, "flock connection error");
                    open = false;
                }
            },
            event = events.recv() => match event {
                Ok(Broadcast::Entries(entries)) => {
                    let message = json!({ "t": "log.entries", "entries": *entries });
                    open = channel.send(&message).await.is_ok();
                    if open {
                        node.peer_has(&member, hashes(&entries));
                    }
                }
                Err(RecvError::Lagged(_)) => {
                    let have = hashes(&node.entries());
                    open = channel.send(&json!({ "t": "log.have", "hashes": have })).await.is_ok();
                }
                Err(RecvError::Closed) => open = false,
            },
            _ = members.changed() => {
                if node.state().is_none_or(|state| state.active(&member).is_none()) {
                    info!(%member, "flock peer removed; closing its connection");
                    let _ = channel.send(&error("removed from the flock")).await;
                    open = false;
                }
            }
            _ = ping.tick() => {
                if last_inbound.elapsed() > PEER_TIMEOUT {
                    debug!(%member, "flock peer timed out");
                    open = false;
                } else {
                    open = channel.send(&json!({ "t": "ping" })).await.is_ok();
                }
            }
            _ = closed.changed() => {
                debug!(%member, "flock connection replaced by a preferred one");
                open = false;
            }
            _ = shutdown.changed() => open = false,
        }
    }
    node.unregister(conn);
    channel.close().await;
    info!(%member, "flock peer disconnected");
}

/// Handles one message on an authenticated connection. `false` closes it.
async fn handle(node: &Node, channel: &mut Channel, member: &str, message: &Value) -> bool {
    match message["t"].as_str() {
        Some("log.have") => {
            let Some(reported) = message["hashes"].as_array() else {
                return true;
            };
            let theirs: HashSet<String> = reported
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let missing: Vec<Entry> = node
                .entries()
                .into_iter()
                .filter(|entry| !theirs.contains(&entry.hash()))
                .collect();
            node.peer_reported(member, theirs);
            let reply = json!({ "t": "log.entries", "entries": missing, "reply": true });
            let sent = channel.send(&reply).await.is_ok();
            if sent {
                node.peer_has(member, hashes(&missing));
            }
            sent
        }
        Some("log.entries") => {
            let Ok(entries) = serde_json::from_value::<Vec<Entry>>(message["entries"].clone())
            else {
                return true;
            };
            if entries.len() > log::MAX_ENTRIES {
                return false;
            }
            node.peer_has(member, hashes(&entries));
            let stored = node.store_entries(&entries);
            if stored > 0 {
                debug!(%member, stored, "flock log entries received");
            }
            if message["reply"] == true {
                node.synced(member);
            }
            true
        }
        Some("ping") => channel.send(&json!({ "t": "pong" })).await.is_ok(),
        Some("error") => {
            if message["reason"] != DUPLICATE {
                warn!(%member, reason = %message["reason"], "flock peer reported an error");
            }
            false
        }
        _ => true,
    }
}

/// A machine joining with an invite: redeem it, show the short code, wait
/// for the owner's decision, then send the log.
async fn join(node: &Arc<Node>, mut channel: Channel, hello: &Value, ip: IpAddr) {
    if node.join_blocked(ip) {
        refuse(channel, "too many failed join attempts; wait a minute").await;
        return;
    }
    let field = |key: &str| hello[key].as_str().unwrap_or_default().to_owned();
    let invite_id = field("invite");
    let sign_key = field("sign_key");
    let name: String = field("name")
        .trim()
        .chars()
        .take(log::MAX_NAME_CHARS)
        .collect();
    let addrs: Vec<String> = hello["addrs"]
        .as_array()
        .map(|addrs| {
            addrs
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let Some(sign_public) = decode_key(&sign_key) else {
        refuse(channel, "signing key is malformed").await;
        return;
    };
    let record = match invite::redeem(node.dir(), &invite_id, &field("secret"), now_unix()) {
        Ok(record) => record,
        Err(reason) => {
            node.record_join_failure(ip);
            refuse(channel, reason).await;
            return;
        }
    };
    let member = MemberInfo {
        id: keys::member_id(&sign_public),
        name: if name.is_empty() {
            "computer".to_owned()
        } else {
            name
        },
        kind: Kind::Computer,
        sign_key,
        noise_key: encode_key(&channel.remote_static),
        addrs,
    };
    let sas = channel::sas(&channel.handshake_hash);
    let request = JoinRequest {
        version: 1,
        invite: invite_id.clone(),
        role: record.role,
        member: member.clone(),
        sas: sas.clone(),
        at: now_unix(),
    };
    if let Err(err) = invite::save_request(node.dir(), &request) {
        warn!(err = %err, "could not save a flock join request");
        refuse(channel, "the host could not record the request").await;
        return;
    }
    info!(member = %member.id, name = %member.name, "flock join waiting for approval");
    if channel
        .send(&json!({ "t": "join.wait", "sas": sas }))
        .await
        .is_err()
    {
        invite::clear(node.dir(), &invite_id);
        return;
    }
    let deadline =
        Instant::now() + Duration::from_secs(record.expires_at.saturating_sub(now_unix()).max(60));
    let mut poll = tokio::time::interval(DECISION_POLL);
    let outcome = loop {
        tokio::select! {
            _ = poll.tick() => {
                if let Some(decision) = invite::load_decision(node.dir(), &invite_id) {
                    break Some(decision);
                }
                if Instant::now() > deadline {
                    break None;
                }
            }
            closed = channel.recv() => {
                if !matches!(closed, Ok(Some(_))) {
                    info!(member = %member.id, "flock join abandoned");
                    invite::clear(node.dir(), &invite_id);
                    return;
                }
            }
        }
    };
    match outcome {
        Some(Decision::Approved) => {
            node.reload();
            if node
                .state()
                .is_none_or(|state| state.active(&member.id).is_none())
            {
                refuse(channel, "approved, but the member is missing from the log").await;
            } else {
                let joined = json!({
                    "t": "joined",
                    "group": node.state().map(|state| state.group).unwrap_or_default(),
                    "entries": node.entries(),
                });
                let _ = channel.send(&joined).await;
                channel.close().await;
                info!(member = %member.id, "flock member joined");
            }
        }
        Some(Decision::Denied) => refuse(channel, "the owner declined the join").await,
        None => refuse(channel, "the join timed out").await,
    }
    invite::clear(node.dir(), &invite_id);
}
