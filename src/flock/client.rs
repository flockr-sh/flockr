//! A CLI's own connection to the hub, for reaching other members' APIs.
//!
//! The CLI dials the hub with this machine's key and says `client`, so the
//! hub doesn't treat it as the server's link. Each API connection becomes a
//! stream: `open {s, to, service?}`, then `data {s, d}` both ways (base64),
//! then `close {s}`. The hub relays the stream to the member `to`, which
//! serves it: the `api` service (the default) from its API socket, the
//! `flock` service (messages) from its flock node.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::channel::Channel;
use super::keys::{self, decode_key, MemberKeys};
use super::pipe::{self, PipeWriter};
use super::store::{self, Member};

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
const PING_INTERVAL: Duration = Duration::from_secs(20);
pub(crate) const API_SERVICE: &str = "api";
pub(crate) const FLOCK_SERVICE: &str = "flock";
/// The TUI client protocol, piped to the member's client socket.
pub(crate) const CLIENT_SERVICE: &str = "client";

pub(crate) fn data_message(stream: u64, bytes: &[u8]) -> Value {
    json!({
        "t": "data",
        "s": stream,
        "d": base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

pub(crate) fn data_bytes(message: &Value) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(message["d"].as_str()?)
        .ok()
}

/// An API error line, for a stream the hub or member closed with a reason.
pub(crate) fn error_line(code: &str, message: &str) -> Vec<u8> {
    let mut line = json!({ "id": "", "error": { "code": code, "message": message } })
        .to_string()
        .into_bytes();
    line.push(b'\n');
    line
}

/// Where and how to reach the hub from this machine.
struct HubRoute {
    group: String,
    me: String,
    hub_key: [u8; 32],
    addrs: Vec<String>,
}

fn hub_route(dir: &Path, keys: &MemberKeys) -> io::Result<HubRoute> {
    if let Some(hub) = store::load_hub(dir)? {
        let node = store::load_node(dir)
            .filter(|node| crate::platform::process_exists(node.pid))
            .ok_or_else(|| io::Error::other("the flock node is not running on this hub"))?;
        return Ok(HubRoute {
            group: hub.group,
            me: hub.hub,
            hub_key: keys.noise_public()?,
            addrs: node.bound,
        });
    }
    let group = store::load_group(dir)?
        .ok_or_else(|| io::Error::other("this machine is not in a flock"))?;
    Ok(HubRoute {
        hub_key: decode_key(&group.hub_key)
            .ok_or_else(|| io::Error::other("the hub key in group.json is malformed"))?,
        group: group.group,
        me: group.member,
        addrs: group.hub_addrs,
    })
}

/// The flock's members as this machine knows them: from `hub.json` on the
/// hub, from the hub's last update elsewhere. Also returns this machine's ID.
pub(crate) fn known_members(dir: &Path) -> io::Result<Option<(Vec<Member>, String)>> {
    if let Some(hub) = store::load_hub(dir)? {
        return Ok(Some((hub.members, hub.hub)));
    }
    let Some(group) = store::load_group(dir)? else {
        return Ok(None);
    };
    let members = store::load_members_cache(dir)
        .map(|cache| cache.members)
        .unwrap_or_default();
    Ok(Some((members, group.member)))
}

/// The member `selector` names (ID or name), other than this machine.
/// `Ok(None)` when this machine is in no flock or no member matches.
pub(crate) fn find_member(dir: &Path, selector: &str) -> Result<Option<Member>, String> {
    let Some((members, me)) = known_members(dir).map_err(|err| err.to_string())? else {
        return Ok(None);
    };
    if let Some(member) = members.iter().find(|member| member.id == selector) {
        return Ok((member.id != me).then(|| member.clone()));
    }
    let mut named = members.iter().filter(|member| member.name == selector);
    let Some(member) = named.next() else {
        return Ok(None);
    };
    if named.next().is_some() {
        return Err(format!(
            "several flock members are named '{selector}'; use the member ID"
        ));
    }
    Ok((member.id != me).then(|| member.clone()))
}

async fn connect(dir: &Path) -> io::Result<Channel> {
    let keys = keys::load_or_create(dir)?;
    let route = hub_route(dir, &keys)?;
    let mut last_error = io::Error::other("the hub has no addresses");
    let mut channel = None;
    for addr in &route.addrs {
        match Channel::connect(addr, &keys.noise, &route.hub_key, &route.group).await {
            Ok(connected) => {
                channel = Some(connected);
                break;
            }
            Err(err) => {
                last_error = io::Error::other(format!("could not reach the hub at {addr}: {err}"))
            }
        }
    }
    let mut channel = channel.ok_or(last_error)?;
    channel
        .send(&json!({ "t": "hello", "member": route.me, "client": true }))
        .await?;
    let welcome = tokio::time::timeout(HELLO_TIMEOUT, channel.recv())
        .await
        .map_err(|_| io::Error::other("the hub didn't answer"))??
        .ok_or_else(|| io::Error::other("the hub closed the connection"))?;
    if welcome["t"] != "welcome" {
        let reason = welcome["reason"].as_str().unwrap_or("unexpected reply");
        return Err(io::Error::other(format!(
            "the hub refused this machine: {reason}"
        )));
    }
    Ok(channel)
}

/// A local socket whose connections go to a flock member's `service`
/// through the hub. `flockr --machine <member>` points its API client here;
/// the TUI connects to a member's client protocol the same way.
pub(crate) struct FlockApiBridge {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    closed: Option<tokio::sync::oneshot::Sender<()>>,
}

impl FlockApiBridge {
    pub(crate) fn start(member: &Member) -> io::Result<Self> {
        Self::start_service(&member.id, API_SERVICE)
    }

    pub(crate) fn start_service(member: &str, service: &str) -> io::Result<Self> {
        let dir = store::flock_dir();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let channel = runtime.block_on(connect(&dir))?;
        let unique = keys::new_id("")?;
        let path = crate::platform::remote_bridge_endpoint_path(
            &format!("flockr-flock-{}-{unique}.sock", std::process::id()),
            &format!("flockr-flock-{}-{unique}.sock", std::process::id()),
        );
        let _ = std::fs::remove_file(&path);
        let listener = crate::ipc::bind_private_local_listener(&path)?;
        crate::ipc::restrict_socket_permissions(&path, 0o600)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (streams_tx, streams) = mpsc::channel(16);
        {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("flock-bridge-accept".into())
                .spawn(move || {
                    use interprocess::local_socket::traits::Listener as _;
                    while !stop.load(Ordering::Acquire) {
                        let Ok(stream) = listener.accept() else {
                            continue;
                        };
                        if stop.load(Ordering::Acquire) {
                            return;
                        }
                        let Ok(piped) = pipe::start(stream) else {
                            continue;
                        };
                        if streams_tx.blocking_send(piped).is_err() {
                            return;
                        }
                    }
                })?;
        }
        let (closed, close) = tokio::sync::oneshot::channel();
        let to = member.to_owned();
        let service = service.to_owned();
        std::thread::Builder::new()
            .name("flock-bridge".into())
            .spawn(move || runtime.block_on(run_bridge(channel, streams, to, service, close)))?;
        Ok(Self {
            path,
            stop,
            closed: Some(closed),
        })
    }

    pub(crate) fn socket_path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FlockApiBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(closed) = self.closed.take() {
            let _ = closed.send(());
        }
        // Wakes the accept thread so it sees `stop` and exits.
        let _ = crate::ipc::connect_local_stream(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

type LocalStreamPair = (PipeWriter, mpsc::Receiver<Vec<u8>>);

async fn run_bridge(
    mut channel: Channel,
    mut streams: mpsc::Receiver<LocalStreamPair>,
    to: String,
    service: String,
    mut close: tokio::sync::oneshot::Receiver<()>,
) {
    let (from_local_tx, mut from_local) = mpsc::channel::<(u64, Option<Vec<u8>>)>(64);
    let mut writers: HashMap<u64, PipeWriter> = HashMap::new();
    let mut next_stream = 1_u64;
    let mut ping = tokio::time::interval(PING_INTERVAL);
    loop {
        tokio::select! {
            accepted = streams.recv() => {
                let Some((writer, mut inbound)) = accepted else {
                    break;
                };
                let stream = next_stream;
                next_stream += 1;
                let open = json!({ "t": "open", "s": stream, "to": to, "service": service });
                if channel.send(&open).await.is_err() {
                    break;
                }
                writers.insert(stream, writer);
                let from_local_tx = from_local_tx.clone();
                tokio::spawn(async move {
                    while let Some(bytes) = inbound.recv().await {
                        if from_local_tx.send((stream, Some(bytes))).await.is_err() {
                            return;
                        }
                    }
                    let _ = from_local_tx.send((stream, None)).await;
                });
            }
            Some((stream, bytes)) = from_local.recv() => {
                let message = match bytes {
                    Some(bytes) => data_message(stream, &bytes),
                    None => {
                        writers.remove(&stream);
                        json!({ "t": "close", "s": stream })
                    }
                };
                if channel.send(&message).await.is_err() {
                    break;
                }
            }
            message = channel.recv() => {
                let Ok(Some(message)) = message else {
                    break;
                };
                let stream = message["s"].as_u64().unwrap_or_default();
                match message["t"].as_str() {
                    Some("data") => {
                        if let (Some(writer), Some(bytes)) = (writers.get(&stream), data_bytes(&message)) {
                            writer.write(bytes);
                        }
                    }
                    Some("close") => {
                        if let Some(writer) = writers.remove(&stream) {
                            if let Some(reason) = message["error"].as_str() {
                                writer.write(error_line("flock_unreachable", reason));
                            }
                        }
                    }
                    Some("ping") => {
                        if channel.send(&json!({ "t": "pong" })).await.is_err() {
                            break;
                        }
                    }
                    Some("error") => break,
                    _ => {}
                }
            }
            _ = ping.tick() => {
                if channel.send(&json!({ "t": "ping" })).await.is_err() {
                    break;
                }
            }
            _ = &mut close => break,
        }
    }
    channel.close().await;
}

/// Sends one API request to each member and returns each answer or the
/// reason there is none.
pub(crate) fn call_each(
    members: &[String],
    request: &Value,
) -> io::Result<Vec<Result<Value, String>>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(call_streams(
        &store::flock_dir(),
        members,
        API_SERVICE,
        request,
    ))
}

/// Sends one request to one member's `service` and returns its answer.
pub(crate) async fn call_one(
    dir: &Path,
    member: &str,
    service: &str,
    request: &Value,
) -> Result<Value, String> {
    call_streams(dir, &[member.to_owned()], service, request)
        .await
        .map_err(|err| err.to_string())?
        .pop()
        .unwrap_or_else(|| Err("no answer".to_owned()))
}

async fn call_streams(
    dir: &Path,
    members: &[String],
    service: &str,
    request: &Value,
) -> io::Result<Vec<Result<Value, String>>> {
    let mut channel = connect(dir).await?;
    let mut line = request.to_string().into_bytes();
    line.push(b'\n');
    for (index, member) in members.iter().enumerate() {
        let stream = index as u64 + 1;
        channel
            .send(&json!({ "t": "open", "s": stream, "to": member, "service": service }))
            .await?;
        channel.send(&data_message(stream, &line)).await?;
    }
    let mut buffers: Vec<Vec<u8>> = vec![Vec::new(); members.len()];
    let mut results: Vec<Option<Result<Value, String>>> = vec![None; members.len()];
    let deadline = tokio::time::Instant::now() + CALL_TIMEOUT;
    while results.iter().any(Option::is_none) {
        let message = match tokio::time::timeout_at(deadline, channel.recv()).await {
            Ok(Ok(Some(message))) => message,
            Ok(Ok(None) | Err(_)) | Err(_) => break,
        };
        let Some(index) = message["s"]
            .as_u64()
            .and_then(|stream| usize::try_from(stream).ok()?.checked_sub(1))
            .filter(|index| *index < members.len())
        else {
            continue;
        };
        match message["t"].as_str() {
            Some("data") => {
                buffers[index].extend(data_bytes(&message).unwrap_or_default());
                if let Some(end) = buffers[index].iter().position(|byte| *byte == b'\n') {
                    results[index] = Some(
                        serde_json::from_slice(&buffers[index][..end])
                            .map_err(|err| format!("bad answer: {err}")),
                    );
                    let _ = channel
                        .send(&json!({ "t": "close", "s": index as u64 + 1 }))
                        .await;
                }
            }
            Some("close") if results[index].is_none() => {
                results[index] = Some(Err(message["error"]
                    .as_str()
                    .unwrap_or("closed without an answer")
                    .to_owned()));
            }
            _ => {}
        }
    }
    channel.close().await;
    Ok(results
        .into_iter()
        .map(|result| result.unwrap_or_else(|| Err("no answer in time".to_owned())))
        .collect())
}
