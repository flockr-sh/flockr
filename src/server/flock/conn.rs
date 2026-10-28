//! Flock connections. Every connection is a member dialing the hub: a
//! member server's link, or a CLI's client connection.
//!
//! The Noise handshake proves the peer holds its static key; the hub looks
//! that key up in `hub.json`. Once in, a link gets the member list and gets
//! it again whenever members or connections change. A removed member's
//! connections close as soon as the hub reads the change.
//!
//! Clients open API streams to members (see `crate::flock::client`). The hub
//! serves streams to itself from its own API and relays the rest to the
//! target member's link under a stream ID of its own, stamped with the
//! caller's member ID and role.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::serve::serve_stream;
use super::{roster_message, Node, Route};
use crate::flock::channel::{self, Channel};
use crate::flock::client::{data_bytes, data_message};
use crate::flock::invite::{self, Decision, JoinRequest};
use crate::flock::keys::{decode_key, encode_key};
use crate::flock::store::{self, GroupFile, Kind, Member, MembersCache, Role};
use crate::mobile::store::now_unix;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const DECISION_POLL: Duration = Duration::from_millis(500);
const PING_INTERVAL: Duration = Duration::from_secs(20);
const PEER_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const NOT_A_MEMBER: &str = "not a member of this flock";
pub(crate) const REMOVED: &str = "removed from the flock";

fn error(reason: &str) -> Value {
    json!({ "t": "error", "reason": reason })
}

fn close(stream: u64, reason: Option<&str>) -> Value {
    match reason {
        Some(reason) => json!({ "t": "close", "s": stream, "error": reason }),
        None => json!({ "t": "close", "s": stream }),
    }
}

async fn refuse(mut channel: Channel, reason: &str) {
    debug!(reason, "flock connection refused");
    let _ = channel.send(&error(reason)).await;
    channel.close().await;
}

/// An authenticated connection on the hub.
struct Accepted {
    channel: Channel,
    member: String,
    client: bool,
}

pub(super) async fn serve_inbound(node: Arc<Node>, stream: TcpStream, peer: SocketAddr) {
    if !node.try_unauthenticated() {
        debug!(%peer, "flock connection dropped: too many waiting");
        return;
    }
    let outcome = accept(&node, stream, peer.ip()).await;
    node.release_unauthenticated();
    if let Some(accepted) = outcome {
        serve_connection(node, accepted).await;
    }
}

async fn accept(node: &Arc<Node>, stream: TcpStream, ip: IpAddr) -> Option<Accepted> {
    let group = node.hub()?.group;
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
            // The CLI may have just removed this member; don't wait for the poll.
            node.reload();
            let hub = node.hub()?;
            let client = hello["client"] == true;
            let key = encode_key(&channel.remote_static);
            let member = match hub.by_noise_key(&key) {
                // The hub's own CLI connects as a client; its server has no link.
                Some(member) if member.id == hub.hub && !client => {
                    refuse(channel, "that is the hub's own key").await;
                    return None;
                }
                Some(member) if hello["member"] == member.id.as_str() => member.id.clone(),
                Some(_) => {
                    refuse(channel, "member ID does not match the key").await;
                    return None;
                }
                None => {
                    refuse(channel, NOT_A_MEMBER).await;
                    return None;
                }
            };
            let welcome = json!({ "t": "welcome", "hub": hub.hub });
            channel.send(&welcome).await.ok()?;
            Some(Accepted {
                channel,
                member,
                client,
            })
        }
        _ => {
            refuse(channel, "expected hello").await;
            None
        }
    }
}

/// Where a client's stream goes.
enum Target {
    /// Served from the hub's own API.
    Hub(mpsc::UnboundedSender<Vec<u8>>),
    /// Relayed to a member's link under the hub's stream ID.
    Relay {
        route: u64,
        link: mpsc::UnboundedSender<Value>,
    },
}

/// The hub's side of a link or a client connection.
async fn serve_connection(node: Arc<Node>, accepted: Accepted) {
    let Accepted {
        mut channel,
        member,
        client,
    } = accepted;
    let (tx, mut outgoing) = mpsc::unbounded_channel::<Value>();
    let (conn, mut closed) = node.register(&member, client, tx.clone());
    let kind = if client { "client" } else { "member" };
    info!(%member, kind, "flock connection opened");
    let mut targets: HashMap<u64, Target> = HashMap::new();
    let mut roster = node.roster();
    if !client {
        roster.mark_changed();
    }
    let mut shutdown = node.shutdown();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    let mut last_inbound = Instant::now();
    let mut open = true;
    while open {
        tokio::select! {
            message = channel.recv() => match message {
                Ok(Some(message)) => {
                    last_inbound = Instant::now();
                    open = match message["t"].as_str() {
                        Some("ping") => channel.send(&json!({ "t": "pong" })).await.is_ok(),
                        Some("error") => false,
                        Some(_) if client => {
                            client_message(&node, &member, &tx, &mut targets, &message);
                            true
                        }
                        Some("data" | "close") => {
                            relay_answer(&node, &message);
                            true
                        }
                        _ => true,
                    };
                }
                Ok(None) => open = false,
                Err(err) => {
                    debug!(%member, err = %err, "flock connection error");
                    open = false;
                }
            },
            Some(message) = outgoing.recv() => {
                open = channel.send(&message).await.is_ok();
            }
            _ = roster.changed() => {
                match node.hub() {
                    Some(hub) if hub.member(&member).is_some() => {
                        if !client {
                            let message = roster_message(&hub, &node.connected());
                            open = channel.send(&message).await.is_ok();
                        }
                    }
                    _ => {
                        info!(%member, "flock member removed; closing its connection");
                        let _ = channel.send(&error(REMOVED)).await;
                        open = false;
                    }
                }
            }
            _ = ping.tick() => {
                if last_inbound.elapsed() > PEER_TIMEOUT {
                    debug!(%member, "flock connection timed out");
                    open = false;
                } else {
                    open = channel.send(&json!({ "t": "ping" })).await.is_ok();
                }
            }
            _ = closed.changed() => {
                debug!(%member, "flock connection replaced by a newer one");
                open = false;
            }
            _ = shutdown.changed() => open = false,
        }
    }
    for target in targets.into_values() {
        if let Target::Relay { route, link } = target {
            node.remove_route(route);
            let _ = link.send(close(route, None));
        }
    }
    node.unregister(conn);
    channel.close().await;
    info!(%member, kind, "flock connection closed");
}

/// Handles a client's `open`, `data` or `close`.
fn client_message(
    node: &Node,
    member: &str,
    tx: &mpsc::UnboundedSender<Value>,
    targets: &mut HashMap<u64, Target>,
    message: &Value,
) {
    let Some(stream) = message["s"].as_u64() else {
        return;
    };
    match message["t"].as_str() {
        Some("open") => {
            if targets.contains_key(&stream) {
                return;
            }
            let to = message["to"].as_str().unwrap_or_default();
            match open_target(node, member, stream, to, tx) {
                Ok(target) => {
                    targets.insert(stream, target);
                }
                Err(reason) => {
                    let _ = tx.send(close(stream, Some(&reason)));
                }
            }
        }
        Some("data") => {
            let Some(bytes) = data_bytes(message) else {
                return;
            };
            match targets.get(&stream) {
                Some(Target::Hub(input)) => {
                    let _ = input.send(bytes);
                }
                Some(Target::Relay { route, link }) => {
                    let _ = link.send(data_message(*route, &bytes));
                }
                None => {}
            }
        }
        Some("close") => {
            if let Some(Target::Relay { route, link }) = targets.remove(&stream) {
                node.remove_route(route);
                let _ = link.send(close(route, None));
            }
        }
        _ => {}
    }
}

fn open_target(
    node: &Node,
    member: &str,
    stream: u64,
    to: &str,
    tx: &mpsc::UnboundedSender<Value>,
) -> Result<Target, String> {
    let hub = node.hub().ok_or("this machine is no longer the hub")?;
    let role = hub.member(member).ok_or(REMOVED)?.role;
    let target = hub
        .member(to)
        .ok_or_else(|| format!("no flock member {to:?}"))?;
    if target.id == hub.hub {
        return Ok(Target::Hub(serve_stream(
            stream,
            member.to_owned(),
            role,
            tx.clone(),
        )));
    }
    let (link_conn, link) = node
        .link(to)
        .ok_or_else(|| format!("{} is not connected to the hub", target.name))?;
    let route = node.add_route(Route {
        client_tx: tx.clone(),
        client_stream: stream,
        link_conn,
    });
    let _ = link.send(json!({ "t": "open", "s": route, "from": member, "role": role }));
    Ok(Target::Relay { route, link })
}

/// Passes a member's answer on a relayed stream back to the client.
fn relay_answer(node: &Node, message: &Value) {
    let Some(route_id) = message["s"].as_u64() else {
        return;
    };
    let route = if message["t"] == "close" {
        node.remove_route(route_id)
    } else {
        node.route(route_id)
    };
    let Some(route) = route else {
        return;
    };
    let mut message = message.clone();
    message["s"] = json!(route.client_stream);
    let _ = route.client_tx.send(message);
}

/// A member's connection to the hub: dial, hello, then keep the member list
/// cached until either side closes.
pub(super) async fn dial_hub(node: Arc<Node>, group: GroupFile) {
    let Some(hub_key) = decode_key(&group.hub_key) else {
        node.dial_failed(Some("the hub key in group.json is malformed".to_owned()));
        return;
    };
    let mut channel = None;
    for addr in &group.hub_addrs {
        match Channel::connect(addr, &node.keys().noise, &hub_key, &group.group).await {
            Ok(connected) => {
                channel = Some(connected);
                break;
            }
            Err(err) => debug!(%addr, err = %err, "flock could not reach the hub"),
        }
    }
    let Some(mut channel) = channel else {
        node.dial_failed(None);
        return;
    };
    let hello = json!({ "t": "hello", "member": group.member });
    if channel.send(&hello).await.is_err() {
        node.dial_failed(None);
        return;
    }
    let welcome = match tokio::time::timeout(HELLO_TIMEOUT, channel.recv()).await {
        Ok(Ok(Some(welcome))) => welcome,
        _ => {
            node.dial_failed(None);
            return;
        }
    };
    if welcome["t"] != "welcome" || welcome["hub"] != group.hub.as_str() {
        let reason = welcome["reason"]
            .as_str()
            .unwrap_or("unexpected reply")
            .to_owned();
        warn!(reason, "the flock hub refused this machine");
        node.dial_failed(Some(reason));
        channel.close().await;
        return;
    }
    node.hub_connected();
    info!("flock connected to the hub");
    let refused = run_hub_link(&node, &mut channel).await;
    channel.close().await;
    if let Some(reason) = &refused {
        warn!(reason, "the flock hub closed this machine's connection");
    } else {
        info!("flock disconnected from the hub");
    }
    node.hub_disconnected(refused);
}

/// Runs a member's open hub connection: keeps the member list and serves
/// API streams the hub relays. Returns the hub's reason when it turned this
/// member away.
async fn run_hub_link(node: &Node, channel: &mut Channel) -> Option<String> {
    let (tx, mut outgoing) = mpsc::unbounded_channel::<Value>();
    let mut served: HashMap<u64, mpsc::UnboundedSender<Vec<u8>>> = HashMap::new();
    let mut shutdown = node.shutdown();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    let mut last_inbound = Instant::now();
    loop {
        tokio::select! {
            message = channel.recv() => match message {
                Ok(Some(message)) => {
                    last_inbound = Instant::now();
                    let stream = message["s"].as_u64().unwrap_or_default();
                    match message["t"].as_str() {
                        Some("members") => save_roster(node, &message),
                        Some("open") => {
                            let from = message["from"].as_str().unwrap_or_default().to_owned();
                            let Ok(role) = serde_json::from_value::<Role>(message["role"].clone()) else {
                                let _ = tx.send(close(stream, Some("unknown role")));
                                continue;
                            };
                            served.insert(stream, serve_stream(stream, from, role, tx.clone()));
                        }
                        Some("data") => {
                            if let (Some(input), Some(bytes)) = (served.get(&stream), data_bytes(&message)) {
                                let _ = input.send(bytes);
                            }
                        }
                        Some("close") => {
                            served.remove(&stream);
                        }
                        Some("ping") => {
                            if channel.send(&json!({ "t": "pong" })).await.is_err() {
                                return None;
                            }
                        }
                        Some("error") => {
                            return Some(message["reason"].as_str().unwrap_or("refused").to_owned());
                        }
                        _ => {}
                    }
                }
                Ok(None) => return None,
                Err(err) => {
                    debug!(err = %err, "flock hub connection error");
                    return None;
                }
            },
            Some(message) = outgoing.recv() => {
                if message["t"] == "close" {
                    if let Some(stream) = message["s"].as_u64() {
                        served.remove(&stream);
                    }
                }
                if channel.send(&message).await.is_err() {
                    return None;
                }
            }
            _ = ping.tick() => {
                if last_inbound.elapsed() > PEER_TIMEOUT {
                    debug!("flock hub timed out");
                    return None;
                }
                if channel.send(&json!({ "t": "ping" })).await.is_err() {
                    return None;
                }
            }
            _ = shutdown.changed() => return None,
        }
    }
}

fn save_roster(node: &Node, message: &Value) {
    let Ok(members) = serde_json::from_value::<Vec<Member>>(message["members"].clone()) else {
        return;
    };
    let connected = serde_json::from_value(message["connected"].clone()).unwrap_or_default();
    let cache = MembersCache {
        version: 1,
        members,
        connected,
        updated_at: now_unix(),
    };
    if let Err(err) = store::save_members_cache(node.dir(), &cache) {
        warn!(err = %err, "could not save the flock member list");
    }
}

/// A machine joining with an invite: redeem it, show the short code, wait
/// for the decision on the hub, then tell the machine its member ID.
async fn join(node: &Arc<Node>, mut channel: Channel, hello: &Value, ip: IpAddr) {
    if node.join_blocked(ip) {
        refuse(channel, "too many failed join attempts; wait a minute").await;
        return;
    }
    let field = |key: &str| hello[key].as_str().unwrap_or_default().to_owned();
    let invite_id = field("invite");
    let name: String = field("name")
        .trim()
        .chars()
        .take(store::MAX_NAME_CHARS)
        .collect();
    let record = match invite::redeem(node.dir(), &invite_id, &field("secret"), now_unix()) {
        Ok(record) => record,
        Err(reason) => {
            node.record_join_failure(ip);
            refuse(channel, reason).await;
            return;
        }
    };
    let noise_key = encode_key(&channel.remote_static);
    let sas = channel::sas(&channel.handshake_hash);
    let request = JoinRequest {
        version: 1,
        invite: invite_id.clone(),
        role: record.role,
        name: if name.is_empty() {
            "computer".to_owned()
        } else {
            name
        },
        kind: Kind::Computer,
        noise_key: noise_key.clone(),
        sas: sas.clone(),
        at: now_unix(),
    };
    if let Err(err) = invite::save_request(node.dir(), &request) {
        warn!(err = %err, "could not save a flock join request");
        refuse(channel, "the hub could not record the request").await;
        return;
    }
    info!(name = %request.name, "flock join waiting for approval");
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
                    info!("flock join abandoned");
                    invite::clear(node.dir(), &invite_id);
                    return;
                }
            }
        }
    };
    match outcome {
        Some(Decision::Approved) => {
            node.reload();
            let joined = node.hub().and_then(|hub| {
                let member = hub.by_noise_key(&noise_key)?;
                Some(json!({
                    "t": "joined",
                    "group": hub.group,
                    "name": hub.name,
                    "member": member.id,
                    "role": member.role,
                    "hub": hub.hub,
                }))
            });
            match joined {
                Some(joined) => {
                    let _ = channel.send(&joined).await;
                    channel.close().await;
                    info!("flock member joined");
                }
                None => refuse(channel, "approved, but the hub has no record of the member").await,
            }
        }
        Some(Decision::Denied) => refuse(channel, "the join was declined").await,
        None => refuse(channel, "the join timed out").await,
    }
    invite::clear(node.dir(), &invite_id);
}
