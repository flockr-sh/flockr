//! The message service: sends, receives and delivers messages between
//! agents (see `crate::flock::message`).
//!
//! The CLI reaches it on `flock.sock`; other members reach it through the
//! hub as the `flock` stream service. Requests and answers are JSON lines
//! shaped like the API's: `{id, method, params}` in, `{id, result}` or
//! `{id, error: {code, message}}` out.
//!
//! The service reads agents and types notices through this server's own API,
//! as the phone gateway does.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::oneshot;
use tracing::{debug, warn};

use crate::flock::client;
use crate::flock::keys;
use crate::flock::message::{
    self, InboxEntry, InboxState, Message, Party, MAX_QUEUED, RATE_PER_MINUTE, TTL_SECS,
};
use crate::flock::pipe::LineBuffer;
use crate::flock::store::Role;
use crate::mobile::store::now_unix;
use crate::server::mobile::api_proxy::ApiCaller;
use crate::server::mobile::RateWindow;

/// How long a notified agent has before it is reminded.
const RENOTIFY_SECS: u64 = 300;
/// A person typed into the agent's pane since it last changed state, so its
/// prompt may hold their draft; the notice waits this long at most.
const PERSON_DRAFT_HOLD_MS: u64 = 600_000;
const DEFAULT_WAIT_MS: u64 = 600_000;
const MAX_WAIT_MS: u64 = 3_600_000;

/// Who is calling the service.
#[derive(Clone, Debug)]
pub(super) enum Caller {
    /// A CLI on this machine, from the pane in `FLOCKR_PANE_ID` if any.
    Local { pane: Option<String> },
    /// Another member, as stamped by the hub.
    Remote { member: String, role: Role },
}

struct Waiter {
    conversation: String,
    /// The waiting sender.
    waiter: Party,
    /// Whom it waits on.
    peer: Party,
    reply: oneshot::Sender<Result<Message, String>>,
}

pub(crate) struct Messages {
    dir: PathBuf,
    api: ApiCaller,
    /// Senders waiting for a reply, by the ID of the message they sent.
    waiters: Mutex<HashMap<String, Waiter>>,
    rates: Mutex<HashMap<String, RateWindow>>,
    canceled: Mutex<HashSet<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A request failure: code and message for the error answer.
struct Failure(&'static str, String);

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self(code, message.into())
    }
}

type Answer = Result<Value, Failure>;

impl Messages {
    pub(super) fn new(dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            dir,
            api: ApiCaller::new(crate::api::socket_path()),
            waiters: Mutex::new(HashMap::new()),
            rates: Mutex::new(HashMap::new()),
            canceled: Mutex::new(HashSet::new()),
        })
    }

    /// Answers one request line.
    pub(super) async fn handle_line(self: &Arc<Self>, line: &[u8], caller: Caller) -> Vec<u8> {
        let request: Value = serde_json::from_slice(line).unwrap_or(Value::Null);
        let id = request["id"].as_str().unwrap_or_default().to_owned();
        let method = request["method"].as_str().unwrap_or_default();
        let params = &request["params"];
        let answer = match (&caller, method) {
            (Caller::Remote { role, .. }, "message.deliver") if *role != Role::Viewer => {
                self.receive(params, &caller).await
            }
            (Caller::Remote { .. }, _) => Err(Failure::new(
                "forbidden",
                format!("{method} is not open to other members"),
            )),
            (Caller::Local { pane }, method) => {
                let pane = pane.clone();
                match method {
                    "message.send" => self.send(params, pane).await,
                    "message.deliver" => self.receive(params, &caller).await,
                    "message.read" => self.read(pane).await,
                    "message.log" => Ok(self.log(params)),
                    "message.pending" => Ok(json!({ "pending": message::pending(&self.dir) })),
                    "message.approve" => self.approve(params).await,
                    "message.deny" => self.deny(params),
                    "message.cancel" => Ok(self.cancel(params)),
                    "message.settings" => Ok(settings()),
                    "message.settings.set" => set_settings(params),
                    _ => Err(Failure::new(
                        "unknown_method",
                        format!("no method {method:?}"),
                    )),
                }
            }
        };
        let response = match answer {
            Ok(result) => json!({ "id": id, "result": result }),
            Err(Failure(code, message)) => {
                json!({ "id": id, "error": { "code": code, "message": message } })
            }
        };
        let mut bytes = response.to_string().into_bytes();
        bytes.push(b'\n');
        bytes
    }

    async fn api(&self, method: &'static str, params: Value) -> Result<Value, String> {
        let api = self.api.clone();
        tokio::task::spawn_blocking(move || api.call(method, params))
            .await
            .map_err(|err| err.to_string())?
            .map_err(|err| err.to_string())
    }

    /// This machine's member ID and name, if it is in a flock.
    fn me(&self) -> Result<(String, String), Failure> {
        let (members, me) = client::known_members(&self.dir)
            .ok()
            .flatten()
            .ok_or_else(|| Failure::new("not_in_flock", "this machine is not in a flock"))?;
        let name = members
            .iter()
            .find(|member| member.id == me)
            .map(|member| member.name.clone())
            .unwrap_or_else(|| crate::config::Config::load().config.flock.display_name());
        Ok((me, name))
    }

    /// The agent `target` (name or pane ID) on this machine as a party, or
    /// `None` when no agent runs there.
    async fn local_agent(&self, target: &str) -> Option<Party> {
        let response = self
            .api("agent.get", json!({ "target": target }))
            .await
            .ok()?;
        let agent = &response["result"]["agent"];
        agent["agent"].as_str()?;
        let (member, machine) = self.me().ok()?;
        let label = agent["name"]
            .as_str()
            .or_else(|| agent["pane_id"].as_str())?
            .to_owned();
        let cwd = agent["cwd"].as_str().map(str::to_owned);
        Some(Party {
            member,
            machine,
            agent: Some(label),
            terminal: agent["terminal_id"].as_str().map(str::to_owned),
            repo: match cwd {
                Some(cwd) => repo_of(cwd).await,
                None => None,
            },
        })
    }

    /// The agent a message to `target` goes to: by name or pane ID, or by
    /// kind ("claude") when exactly one agent of that kind runs here.
    async fn recipient(&self, target: &str, machine: &str) -> Result<Party, Failure> {
        if let Some(party) = self.local_agent(target).await {
            return Ok(party);
        }
        let listed = self.api("agent.list", json!({})).await.unwrap_or_default();
        match agents_of_kind(&listed["result"]["agents"], target).as_slice() {
            [pane] => self.local_agent(pane).await.ok_or_else(|| {
                Failure::new("no_such_agent", format!("no agent {target:?} on {machine}"))
            }),
            [] => Err(Failure::new(
                "no_such_agent",
                format!("no agent {target:?} on {machine}"),
            )),
            several => Err(Failure::new(
                "ambiguous_agent",
                format!(
                    "{} {target} agents on {machine}; send to one of: {}",
                    several.len(),
                    several
                        .iter()
                        .map(|pane| format!("{machine}:{pane}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        }
    }

    /// The sender: the agent in the calling pane, or a person.
    async fn sender(&self, pane: Option<String>) -> Result<Party, Failure> {
        if let Some(pane) = &pane {
            if let Some(party) = self.local_agent(pane).await {
                return Ok(party);
            }
        }
        let (member, machine) = self.me()?;
        Ok(Party {
            member,
            machine,
            agent: None,
            terminal: None,
            repo: None,
        })
    }

    async fn send(self: &Arc<Self>, params: &Value, pane: Option<String>) -> Answer {
        let from = self.sender(pane).await?;
        let body = params["body"].as_str().unwrap_or_default().to_owned();
        let reply_to = params["reply_to"].as_str().map(str::to_owned);
        let parent = match &reply_to {
            Some(id) => {
                let entry = message::find_received(&self.dir, id).ok_or_else(|| {
                    Failure::new("no_such_message", format!("no received message {id}"))
                })?;
                if !entry.message.to.same_agent(&from) {
                    return Err(Failure::new(
                        "not_yours",
                        format!("message {id} was sent to another agent"),
                    ));
                }
                // Replying reads it, so the agent isn't told about it later.
                if entry.state != InboxState::Read {
                    let terminal = entry.message.to.terminal.clone().unwrap_or_default();
                    let read = InboxEntry {
                        state: InboxState::Read,
                        ..entry.clone()
                    };
                    if message::save_entry(&self.dir, &terminal, &read).is_ok() {
                        message::audit(&self.dir, "read", &entry.message, Some("replied"));
                    }
                }
                Some(entry.message)
            }
            None => None,
        };
        let to = match (params["to"].as_str().filter(|to| !to.is_empty()), &parent) {
            (Some(to), _) => self.address(to)?,
            (None, Some(parent)) => parent.from.clone(),
            (None, None) => return Err(Failure::new("bad_request", "say whom to send it to")),
        };
        if to.agent.is_none() {
            return Err(Failure::new(
                "bad_request",
                "messages go to agents; name one",
            ));
        }
        if from.same_agent(&to) {
            return Err(Failure::new("bad_request", "an agent can't message itself"));
        }
        let conversation = match &parent {
            Some(parent) => parent.conversation.clone(),
            None => keys::new_id("c_").map_err(|err| Failure::new("internal", err.to_string()))?,
        };
        if lock(&self.canceled).contains(&conversation) {
            return Err(Failure::new(
                "canceled",
                "a person canceled this conversation",
            ));
        }
        let wait = params["wait"] == true;
        let now = now_unix();
        let mut message = Message {
            id: keys::new_id("msg_").map_err(|err| Failure::new("internal", err.to_string()))?,
            conversation,
            reply_to,
            hops: parent.as_ref().map_or(0, |parent| parent.hops + 1),
            wait,
            sent_at: now,
            expires_at: now + TTL_SECS,
            from,
            to,
            body,
        };
        message::check_limits(&message, now).map_err(|reason| Failure::new("refused", reason))?;
        let reply = wait.then(|| {
            let (reply, waiting) = oneshot::channel();
            lock(&self.waiters).insert(
                message.id.clone(),
                Waiter {
                    conversation: message.conversation.clone(),
                    waiter: message.from.clone(),
                    peer: message.to.clone(),
                    reply,
                },
            );
            waiting
        });
        let delivered = self.deliver(&message).await;
        let status = match delivered {
            Ok((status, agent)) => {
                // Keep the agent it reached, so its reply matches.
                if agent.is_some() {
                    message.to.agent = agent;
                    if let Some(waiter) = lock(&self.waiters).get_mut(&message.id) {
                        waiter.peer = message.to.clone();
                    }
                }
                status
            }
            Err(reason) => {
                lock(&self.waiters).remove(&message.id);
                message::audit(&self.dir, "refused", &message, Some(&reason));
                return Err(Failure::new("refused", reason));
            }
        };
        message::audit(&self.dir, "sent", &message, Some(&status));
        if let Err(err) = message::save_sent(&self.dir, &message, now) {
            tracing::warn!(err = %err, "could not keep the sent message");
        }
        let mut result = json!({
            "id": message.id,
            "conversation": message.conversation,
            "to": message.to.label(),
            "status": status,
        });
        if let Some(waiting) = reply {
            let timeout = params["timeout_ms"]
                .as_u64()
                .unwrap_or(DEFAULT_WAIT_MS)
                .min(MAX_WAIT_MS);
            let outcome = tokio::time::timeout(Duration::from_millis(timeout), waiting).await;
            lock(&self.waiters).remove(&message.id);
            match outcome {
                Ok(Ok(Ok(reply))) => result["reply"] = json!(reply),
                Ok(Ok(Err(reason))) => return Err(Failure::new("canceled", reason)),
                Ok(Err(_)) | Err(_) => {
                    return Err(Failure::new(
                        "timeout",
                        format!("no reply to {} in time", message.id),
                    ))
                }
            }
        }
        Ok(result)
    }

    /// `machine:agent` or `agent` (this machine) as a recipient party.
    fn address(&self, to: &str) -> Result<Party, Failure> {
        let (me, my_name) = self.me()?;
        let (machine, agent) = match to.split_once(':') {
            // Pane IDs (`w1:p2`) contain a colon too; a known machine name
            // or member ID before it decides.
            Some((machine, agent)) if !machine.is_empty() => {
                match client::find_member(&self.dir, machine) {
                    Ok(Some(member)) => (Some(member), agent),
                    Ok(None) if machine == my_name || machine == me => (None, agent),
                    Ok(None) => (None, to),
                    Err(reason) => return Err(Failure::new("bad_request", reason)),
                }
            }
            _ => (None, to),
        };
        let (member, machine) = match machine {
            Some(member) => (member.id, member.name),
            None => (me, my_name),
        };
        Ok(Party {
            member,
            machine,
            agent: Some(agent.to_owned()),
            terminal: None,
            repo: None,
        })
    }

    /// Hands a message to the recipient's machine. Returns its status there.
    /// Delivers `message` and returns its status and the agent it reached:
    /// the recipient's name or pane, which can differ from the address
    /// ("claude") the sender used.
    async fn deliver(
        self: &Arc<Self>,
        message: &Message,
    ) -> Result<(String, Option<String>), String> {
        let (me, _) = self.me().map_err(|Failure(_, reason)| reason)?;
        let request = json!({
            "id": "flock:message:deliver",
            "method": "message.deliver",
            "params": { "message": message },
        });
        let response = if message.to.member == me {
            let line = request.to_string();
            let answer = self.handle_line(line.as_bytes(), Caller::Local { pane: None });
            serde_json::from_slice::<Value>(&Box::pin(answer).await)
                .map_err(|err| err.to_string())?
        } else {
            client::call_one(
                &self.dir,
                &message.to.member,
                client::FLOCK_SERVICE,
                &request,
            )
            .await?
        };
        if let Some(reason) = response["error"]["message"].as_str() {
            return Err(reason.to_owned());
        }
        let status = response["result"]["status"]
            .as_str()
            .unwrap_or("delivered")
            .to_owned();
        let agent = response["result"]["agent"].as_str().map(str::to_owned);
        Ok((status, agent))
    }

    /// `message.deliver` on the recipient's machine.
    async fn receive(self: &Arc<Self>, params: &Value, caller: &Caller) -> Answer {
        let mut message: Message = serde_json::from_value(params["message"].clone())
            .map_err(|err| Failure::new("bad_request", err.to_string()))?;
        let (me, _) = self.me()?;
        let stamped = match caller {
            Caller::Remote { member, .. } => member.clone(),
            Caller::Local { .. } => me.clone(),
        };
        if message.from.member != stamped {
            return Err(Failure::new(
                "forbidden",
                "the sender's machine doesn't match the connection",
            ));
        }
        if message.to.member != me || !message::valid_id(&message.id) {
            return Err(Failure::new(
                "bad_request",
                "the message isn't for this machine",
            ));
        }
        message::check_limits(&message, now_unix())
            .map_err(|reason| Failure::new("refused", reason))?;
        if lock(&self.canceled).contains(&message.conversation) {
            return Err(Failure::new(
                "canceled",
                "a person canceled this conversation",
            ));
        }
        let rate_key = format!("{}:{}", message.from.member, message.from.label());
        let within_rate = lock(&self.rates).entry(rate_key).or_default().try_hit(
            Instant::now(),
            RATE_PER_MINUTE,
            Duration::from_secs(60),
        );
        if !within_rate {
            return Err(Failure::new(
                "rate_limited",
                format!("more than {RATE_PER_MINUTE} messages a minute to this machine"),
            ));
        }
        let target = message.to.agent.clone().unwrap_or_default();
        message.to = self.recipient(&target, &message.to.machine).await?;
        let terminal = message.to.terminal.clone().unwrap_or_default();
        let inbox = message::inbox(&self.dir, &terminal);
        if inbox.iter().any(|entry| entry.message.id == message.id) {
            return Ok(json!({ "status": "duplicate", "agent": message.to.agent }));
        }
        if message.wait && self.waiting_on(&message.to, &message.from) {
            return Err(Failure::new(
                "wait_cycle",
                format!(
                    "{} is already waiting on {}; both would wait forever",
                    message.to.label(),
                    message.from.label()
                ),
            ));
        }
        let unread = inbox
            .iter()
            .filter(|entry| entry.state != InboxState::Read)
            .count();
        if unread >= MAX_QUEUED {
            return Err(Failure::new(
                "inbox_full",
                format!("{} has {MAX_QUEUED} unread messages", message.to.label()),
            ));
        }
        // A reply someone waits for goes straight to them.
        let waiter = message
            .reply_to
            .as_ref()
            .and_then(|id| lock(&self.waiters).remove(id));
        if let Some(waiter) = waiter {
            self.store(&message, InboxState::Read)?;
            message::audit(
                &self.dir,
                "received",
                &message,
                Some("taken by the waiting sender"),
            );
            let agent = message.to.agent.clone();
            let _ = waiter.reply.send(Ok(message));
            return Ok(json!({ "status": "delivered", "agent": agent }));
        }
        let mode = crate::config::Config::load().config.flock.approve_messages;
        if !message::allowed(&self.dir, &message, mode) {
            message::save_pending(&self.dir, &message)
                .map_err(|err| Failure::new("internal", err.to_string()))?;
            message::audit(&self.dir, "pending", &message, None);
            self.ask_approval(&message).await;
            return Ok(json!({ "status": "pending_approval", "agent": message.to.agent }));
        }
        self.store(&message, InboxState::Queued)?;
        message::audit(&self.dir, "received", &message, None);
        Ok(json!({ "status": "queued", "agent": message.to.agent }))
    }

    fn waiting_on(&self, waiter: &Party, peer: &Party) -> bool {
        lock(&self.waiters)
            .values()
            .any(|entry| entry.waiter.same_agent(waiter) && entry.peer.same_agent(peer))
    }

    fn store(&self, message: &Message, state: InboxState) -> Result<(), Failure> {
        let terminal = message.to.terminal.clone().unwrap_or_default();
        let entry = InboxEntry {
            message: message.clone(),
            state,
            notified_at: None,
        };
        message::save_entry(&self.dir, &terminal, &entry)
            .map_err(|err| Failure::new("internal", err.to_string()))
    }

    async fn ask_approval(&self, message: &Message) {
        let body = format!(
            "{} to {}. Run `flockr message approve {}` (add --always to allow them from now on) or `flockr message deny {}`.",
            message.from.label(),
            message.to.label(),
            message.id,
            message.id
        );
        let params =
            json!({ "title": "Agent message needs approval", "body": body, "sound": "request" });
        if let Err(err) = self.api("notification.show", params).await {
            debug!(err, "could not show the message approval notice");
        }
    }

    /// `message.read`: the calling agent's unread messages, now marked read.
    async fn read(&self, pane: Option<String>) -> Answer {
        let pane = pane.ok_or_else(|| {
            Failure::new("bad_request", "run this from the agent's pane in flockr")
        })?;
        let me = self
            .local_agent(&pane)
            .await
            .ok_or_else(|| Failure::new("bad_request", "no agent runs in this pane"))?;
        let terminal = me.terminal.clone().unwrap_or_default();
        let mut messages = Vec::new();
        for mut entry in message::inbox(&self.dir, &terminal) {
            if entry.state == InboxState::Read {
                continue;
            }
            entry.state = InboxState::Read;
            if let Err(err) = message::save_entry(&self.dir, &terminal, &entry) {
                warn!(err = %err, "could not mark a message read");
            }
            message::audit(&self.dir, "read", &entry.message, None);
            messages.push(entry.message);
        }
        Ok(json!({ "messages": messages }))
    }

    fn log(&self, params: &Value) -> Value {
        let limit = params["limit"].as_u64().unwrap_or(50).min(1000) as usize;
        json!({ "events": message::audit_tail(&self.dir, limit) })
    }

    async fn approve(self: &Arc<Self>, params: &Value) -> Answer {
        let id = params["id"].as_str().unwrap_or_default();
        let message = message::take_pending(&self.dir, id)
            .ok_or_else(|| Failure::new("no_such_message", format!("no pending message {id}")))?;
        if params["always"] == true {
            if let Some(rule) = message::rule_for(&message) {
                message::add_rule(&self.dir, rule)
                    .map_err(|err| Failure::new("internal", err.to_string()))?;
            }
        }
        self.store(&message, InboxState::Queued)?;
        message::audit(&self.dir, "approved", &message, None);
        Ok(json!({ "id": message.id, "to": message.to.label() }))
    }

    fn deny(&self, params: &Value) -> Answer {
        let id = params["id"].as_str().unwrap_or_default();
        let message = message::take_pending(&self.dir, id)
            .ok_or_else(|| Failure::new("no_such_message", format!("no pending message {id}")))?;
        message::audit(&self.dir, "denied", &message, None);
        Ok(json!({ "id": message.id }))
    }

    /// `message.cancel`: refuses the conversation from now on on this
    /// machine, drops its unread messages and wakes anyone waiting in it.
    fn cancel(&self, params: &Value) -> Value {
        let conversation = params["conversation"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        lock(&self.canceled).insert(conversation.clone());
        let mut dropped = 0;
        for terminal in message::inbox_terminals(&self.dir) {
            for entry in message::inbox(&self.dir, &terminal) {
                if entry.message.conversation == conversation && entry.state != InboxState::Read {
                    message::remove_entry(&self.dir, &terminal, &entry.message.id);
                    message::audit(&self.dir, "canceled", &entry.message, None);
                    dropped += 1;
                }
            }
        }
        for pending in message::pending(&self.dir) {
            if pending.conversation == conversation {
                message::take_pending(&self.dir, &pending.id);
                message::audit(&self.dir, "canceled", &pending, None);
                dropped += 1;
            }
        }
        let woken: Vec<Waiter> = {
            let mut waiters = lock(&self.waiters);
            let ids: Vec<String> = waiters
                .iter()
                .filter(|(_, waiter)| waiter.conversation == conversation)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| waiters.remove(id)).collect()
        };
        let woken_count = woken.len();
        for waiter in woken {
            let _ = waiter.reply.send(Err(format!(
                "a person canceled conversation {conversation}"
            )));
        }
        json!({ "conversation": conversation, "dropped": dropped, "woken": woken_count })
    }

    /// Tells idle recipients about unread messages and drops expired ones.
    pub(super) async fn deliver_due(&self) {
        let now = now_unix();
        for terminal in message::inbox_terminals(&self.dir) {
            let entries = message::inbox(&self.dir, &terminal);
            let mut due: Vec<InboxEntry> = Vec::new();
            for entry in entries {
                if entry.state == InboxState::Read {
                    if entry.message.expires_at + TTL_SECS <= now {
                        message::remove_entry(&self.dir, &terminal, &entry.message.id);
                    }
                    continue;
                }
                if entry.message.expires_at <= now {
                    message::audit(&self.dir, "expired", &entry.message, None);
                    message::remove_entry(&self.dir, &terminal, &entry.message.id);
                    continue;
                }
                let remind = entry
                    .notified_at
                    .is_none_or(|at| now.saturating_sub(at) >= RENOTIFY_SECS);
                if remind {
                    due.push(entry);
                }
            }
            if due.is_empty() {
                continue;
            }
            let target = due[0].message.to.agent.clone().unwrap_or_default();
            if !self.idle(&target, &terminal).await {
                continue;
            }
            let senders: Vec<String> = {
                let mut senders: Vec<String> =
                    due.iter().map(|entry| entry.message.from.label()).collect();
                senders.dedup();
                senders
            };
            let notice = format!(
                "[flockr] {} new message{} from {}. Read and act on {} with: flockr message read",
                due.len(),
                if due.len() == 1 { "" } else { "s" },
                senders.join(", "),
                if due.len() == 1 { "it" } else { "them" },
            );
            match self
                .api("agent.prompt", json!({ "target": target, "text": notice }))
                .await
            {
                Ok(_) => {
                    for mut entry in due {
                        entry.state = InboxState::Notified;
                        entry.notified_at = Some(now);
                        if message::save_entry(&self.dir, &terminal, &entry).is_ok() {
                            message::audit(&self.dir, "notified", &entry.message, None);
                        }
                    }
                }
                Err(err) => debug!(err, %target, "could not notify an agent of its messages"),
            }
        }
    }

    /// Whether the agent is cleanly idle: idle or done, in the same terminal
    /// the messages were sent to, and without a person's unsent draft.
    async fn idle(&self, target: &str, terminal: &str) -> bool {
        let Ok(response) = self.api("agent.get", json!({ "target": target })).await else {
            return false;
        };
        let agent = &response["result"]["agent"];
        let person_typing = agent["person_input_pending"] == true
            && agent["person_input_idle_ms"]
                .as_u64()
                .is_some_and(|idle| idle < PERSON_DRAFT_HOLD_MS);
        agent["terminal_id"] == terminal
            && matches!(agent["agent_status"].as_str(), Some("idle" | "done"))
            && !person_typing
    }
}

/// The normalized `origin` remote of the repo at `cwd`.
async fn repo_of(cwd: String) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(["-C", &cwd, "remote", "get-url", "origin"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    message::normalize_repo(&String::from_utf8_lossy(&output.stdout))
}

/// Serves `flock.sock` for the CLI on this machine.
pub(super) fn serve_local(messages: Arc<Messages>, shutdown: tokio::sync::watch::Receiver<bool>) {
    let path = message::socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = match crate::ipc::bind_private_local_listener(&path) {
        Ok(listener) => listener,
        Err(err) => {
            warn!(path = %path.display(), err = %err, "flock could not open its message socket");
            return;
        }
    };
    if let Err(err) = crate::ipc::restrict_socket_permissions(&path, 0o600) {
        warn!(err = %err, "could not restrict the flock message socket");
    }
    // After a live handoff the new server owns this path; removing it on the
    // way out would cut the new server's agents off from messages.
    let identity = crate::ipc::socket_file_identity(&path).ok();
    let (accepted_tx, mut accepted) = tokio::sync::mpsc::channel(16);
    let spawned = std::thread::Builder::new()
        .name("flock-socket".into())
        .spawn(move || {
            use interprocess::local_socket::traits::Listener as _;
            loop {
                let Ok(stream) = listener.accept() else {
                    continue;
                };
                let Ok(piped) = crate::flock::pipe::start(stream) else {
                    continue;
                };
                if accepted_tx.blocking_send(piped).is_err() {
                    return;
                }
            }
        });
    if let Err(err) = spawned {
        warn!(err = %err, "flock could not start its message socket");
        return;
    }
    let mut shutdown = shutdown;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some((writer, mut inbound)) = accepted.recv() => {
                    let messages = messages.clone();
                    tokio::spawn(async move {
                        let mut lines = LineBuffer::default();
                        while let Some(bytes) = inbound.recv().await {
                            for line in lines.push(&bytes) {
                                let pane = serde_json::from_slice::<Value>(&line)
                                    .ok()
                                    .and_then(|request| request["params"]["caller_pane"].as_str().map(str::to_owned));
                                let answer = messages.handle_line(&line, Caller::Local { pane }).await;
                                if !writer.write(answer) {
                                    return;
                                }
                            }
                        }
                    });
                }
                _ = shutdown.changed() => {
                    if let Some(identity) = &identity {
                        let _ = crate::ipc::remove_socket_file_if_owned(&path, identity);
                    }
                    return;
                }
            }
        }
    });
}

/// `message.settings`: this machine's message settings.
fn settings() -> Value {
    json!({ "approve_messages": crate::config::Config::load().config.flock.approve_messages })
}

/// `message.settings.set`: changes `[flock] approve_messages` in this
/// machine's config. It applies to the next message.
fn set_settings(params: &Value) -> Answer {
    let mode: crate::config::ApproveMessages =
        serde_json::from_value(params["approve_messages"].clone()).map_err(|_| {
            Failure::new(
                "bad_request",
                "approve_messages is \"other-projects\", \"never\" or \"always\"",
            )
        })?;
    let literal =
        serde_json::to_string(&mode).map_err(|err| Failure::new("internal", err.to_string()))?;
    crate::config::write_edit(crate::config::ConfigEdit::Set {
        table: "flock",
        key: "approve_messages",
        value: &literal,
    })
    .map_err(|err| Failure::new("internal", err))?;
    Ok(settings())
}

/// The names (or pane IDs) of the listed agents of `kind`.
fn agents_of_kind(agents: &Value, kind: &str) -> Vec<String> {
    agents
        .as_array()
        .into_iter()
        .flatten()
        .filter(|agent| agent["agent"].as_str() == Some(kind))
        .filter_map(|agent| {
            agent["name"]
                .as_str()
                .or_else(|| agent["pane_id"].as_str())
                .map(str::to_owned)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flock::message::tests::party;

    #[test]
    fn agents_are_found_by_kind_under_their_name_or_pane() {
        let agents = json!([
            { "agent": "claude", "pane_id": "w3:p1" },
            { "agent": "codex", "pane_id": "w3:p2", "name": "reviewer" },
            { "agent": "codex", "pane_id": "w4:p1" },
        ]);
        assert_eq!(agents_of_kind(&agents, "claude"), vec!["w3:p1"]);
        assert_eq!(agents_of_kind(&agents, "codex"), vec!["reviewer", "w4:p1"]);
        assert!(agents_of_kind(&agents, "pi").is_empty());
    }

    #[test]
    fn lines_split_across_reads_come_out_whole() {
        let mut lines = LineBuffer::default();
        assert!(lines.push(b"{\"a\":").is_empty());
        assert_eq!(
            lines.push(b"1}\n{\"b\":2}\n{"),
            vec![b"{\"a\":1}\n".to_vec(), b"{\"b\":2}\n".to_vec()]
        );
    }

    #[test]
    fn a_wait_cycle_is_seen_from_the_recipient_side() {
        let messages = Messages::new(PathBuf::new());
        let worker = party("m_b", Some("worker"), None);
        let boss = party("m_a", Some("boss"), None);
        let (reply, _waiting) = oneshot::channel();
        lock(&messages.waiters).insert(
            "msg_1".into(),
            Waiter {
                conversation: "c_1".into(),
                waiter: worker.clone(),
                peer: boss.clone(),
                reply,
            },
        );
        assert!(messages.waiting_on(&worker, &boss));
        assert!(!messages.waiting_on(&boss, &worker));
        let canceled = messages.cancel(&json!({ "conversation": "c_1" }));
        assert_eq!(canceled["woken"], 1);
        assert!(!messages.waiting_on(&worker, &boss));
    }
}
