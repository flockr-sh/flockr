//! Blocked/Done alerts (§11): trigger, confirm, dedupe, then Expo push or an
//! in-band `notify` for apps in the foreground.
//!
//! The trigger state machine is pure and takes `now` as an argument. The
//! worker and the Expo sender run on their own threads, never on the server
//! loop or a tokio worker.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tracing::{debug, warn};

use super::api_proxy::result_field;
use super::{Gateway, RateWindow, PUSHES_PER_DEVICE_PER_MINUTE};
use crate::api::schema::{AgentStatus, EventData};
use crate::mobile::store;

const POLL: Duration = Duration::from_millis(250);
const DEDUPE_KEYS: usize = 1024;
const EXPO_URL: &str = "https://exp.host/--/api/v2/push/send";
const EXPO_BATCH: usize = 100;
const EXPO_QUEUE: usize = 256;
const EXPO_RETRIES: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(8)];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AlertKind {
    Blocked,
    Done,
}

impl AlertKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Done => "done",
        }
    }

    fn matches(self, status: AgentStatus) -> bool {
        matches!(
            (self, status),
            (Self::Blocked, AgentStatus::Blocked) | (Self::Done, AgentStatus::Done)
        )
    }
}

/// Status transitions → delayed checks → deduped alerts.
pub(crate) struct PushTrigger {
    blocked: bool,
    done: bool,
    delay: Duration,
    last_status: HashMap<String, AgentStatus>,
    pending: HashMap<String, (AlertKind, Instant)>,
    sent_order: VecDeque<(String, AlertKind, u64)>,
    sent: HashSet<(String, AlertKind, u64)>,
}

impl PushTrigger {
    pub(crate) fn new(blocked: bool, done: bool, delay: Duration) -> Self {
        Self {
            blocked,
            done,
            delay,
            last_status: HashMap::new(),
            pending: HashMap::new(),
            sent_order: VecDeque::new(),
            sent: HashSet::new(),
        }
    }

    /// Records one `pane.agent_status_changed` event.
    pub(crate) fn on_status(&mut self, pane_id: &str, status: AgentStatus, now: Instant) {
        if self.last_status.get(pane_id) == Some(&status) {
            return;
        }
        self.last_status.insert(pane_id.to_owned(), status);
        let kind = match status {
            AgentStatus::Blocked if self.blocked => Some(AlertKind::Blocked),
            AgentStatus::Done if self.done => Some(AlertKind::Done),
            _ => None,
        };
        match kind {
            Some(kind) => {
                self.pending
                    .insert(pane_id.to_owned(), (kind, now + self.delay));
            }
            None => {
                self.pending.remove(pane_id);
            }
        }
    }

    /// Checks whose confirm delay has passed.
    pub(crate) fn take_due(&mut self, now: Instant) -> Vec<(String, AlertKind)> {
        let due = self
            .pending
            .iter()
            .filter(|(_, (_, at))| *at <= now)
            .map(|(pane, (kind, _))| (pane.clone(), *kind))
            .collect::<Vec<_>>();
        for (pane, _) in &due {
            self.pending.remove(pane);
        }
        due
    }

    /// Whether the confirmed status still matches the scheduled alert.
    pub(crate) fn confirms(kind: AlertKind, current: AgentStatus) -> bool {
        kind.matches(current)
    }

    /// `true` the first time `(pane, kind, state_change_seq)` is seen.
    pub(crate) fn first_send(&mut self, pane_id: &str, kind: AlertKind, seq: u64) -> bool {
        let key = (pane_id.to_owned(), kind, seq);
        if !self.sent.insert(key.clone()) {
            return false;
        }
        self.sent_order.push_back(key);
        while self.sent_order.len() > DEDUPE_KEYS {
            if let Some(old) = self.sent_order.pop_front() {
                self.sent.remove(&old);
            }
        }
        true
    }
}

/// Alert text and ids for one confirmed transition.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Alert {
    pub kind: AlertKind,
    pub title: String,
    pub body: String,
    pub agent: String,
    pub workspace: String,
    pub tab: String,
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub terminal_id: String,
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn label_for(list: &Value, id_key: &str, id: &str) -> Option<String> {
    list.as_array()?
        .iter()
        .find(|item| item.get(id_key).and_then(Value::as_str) == Some(id))
        .and_then(|item| str_field(item, "label"))
        .map(str::to_owned)
}

/// Builds the alert from an `AgentInfo` JSON object and the label lists.
pub(crate) fn build_alert(
    kind: AlertKind,
    agent: &Value,
    workspaces: &Value,
    tabs: &Value,
    host: &str,
) -> Alert {
    let name = str_field(agent, "display_agent")
        .or_else(|| str_field(agent, "name"))
        .or_else(|| str_field(agent, "agent"))
        .unwrap_or("Agent")
        .to_owned();
    let workspace_id = str_field(agent, "workspace_id")
        .unwrap_or_default()
        .to_owned();
    let tab_id = str_field(agent, "tab_id").unwrap_or_default().to_owned();
    let workspace = label_for(workspaces, "workspace_id", &workspace_id)
        .unwrap_or_else(|| workspace_id.clone());
    let tab = label_for(tabs, "tab_id", &tab_id).unwrap_or_else(|| tab_id.clone());
    let title = match kind {
        AlertKind::Blocked => format!("{name} is blocked"),
        AlertKind::Done => format!("{name} finished"),
    };
    Alert {
        kind,
        body: format!("{workspace} › {tab} · {host}"),
        title,
        agent: name,
        workspace,
        tab,
        pane_id: str_field(agent, "pane_id").unwrap_or_default().to_owned(),
        workspace_id,
        tab_id,
        terminal_id: str_field(agent, "terminal_id")
            .unwrap_or_default()
            .to_owned(),
    }
}

pub(crate) fn notify_message(alert: &Alert) -> String {
    json!({
        "t": "notify",
        "kind": alert.kind.as_str(),
        "pane_id": alert.pane_id,
        "workspace_id": alert.workspace_id,
        "tab_id": alert.tab_id,
        "terminal_id": alert.terminal_id,
        "title": alert.title,
        "body": alert.body,
    })
    .to_string()
}

pub(crate) fn expo_message(alert: &Alert, to: &str, server_id: &str, server_name: &str) -> Value {
    json!({
        "to": to,
        "title": alert.title,
        "body": alert.body,
        "sound": "default",
        "priority": "high",
        "channelId": "agents",
        "interruptionLevel": match alert.kind {
            AlertKind::Blocked => "time-sensitive",
            AlertKind::Done => "active",
        },
        "ttl": 3600,
        "data": {
            "type": "agent_status",
            "server_id": server_id,
            "server_name": server_name,
            "status": alert.kind.as_str(),
            "agent": alert.agent,
            "workspace_id": alert.workspace_id,
            "workspace": alert.workspace,
            "tab_id": alert.tab_id,
            "tab": alert.tab,
            "pane_id": alert.pane_id,
            "terminal_id": alert.terminal_id,
        }
    })
}

/// Starts the trigger worker thread.
pub(crate) fn start_worker(gateway: Arc<Gateway>) {
    let spawned = crate::thread_spawn::spawn_named("flockr-mobile-push", move || worker(gateway));
    if let Err(err) = spawned {
        warn!(err = %err, "could not start mobile push worker");
    }
}

fn worker(gateway: Arc<Gateway>) {
    let mut trigger = PushTrigger::new(
        gateway.push.blocked,
        gateway.push.done,
        gateway.push.confirm_delay(),
    );
    let mut limiter: HashMap<String, RateWindow> = HashMap::new();
    let mut sequence = gateway.event_hub.current_sequence();
    while !gateway.stopped.load(Ordering::Acquire) {
        std::thread::sleep(POLL);
        let now = Instant::now();
        if gateway.event_hub.current_sequence() != sequence {
            for (event_sequence, envelope) in gateway.event_hub.events_after(sequence) {
                sequence = sequence.max(event_sequence);
                if let EventData::PaneAgentStatusChanged {
                    pane_id,
                    agent_status: status,
                    ..
                } = &envelope.data
                {
                    trigger.on_status(pane_id, *status, now);
                }
            }
        }
        for (pane_id, kind) in trigger.take_due(now) {
            confirm_and_send(&gateway, &mut trigger, &mut limiter, &pane_id, kind);
        }
    }
}

fn confirm_and_send(
    gateway: &Gateway,
    trigger: &mut PushTrigger,
    limiter: &mut HashMap<String, RateWindow>,
    pane_id: &str,
    kind: AlertKind,
) {
    let agent = match gateway.api.call("agent.get", json!({ "target": pane_id })) {
        Ok(response) => match result_field(&response, "agent") {
            Some(agent) => agent.clone(),
            None => return,
        },
        Err(err) => {
            debug!(pane_id, err = %err, "push confirm: agent.get failed");
            return;
        }
    };
    let status = agent
        .get("agent_status")
        .cloned()
        .and_then(|value| serde_json::from_value::<AgentStatus>(value).ok())
        .unwrap_or(AgentStatus::Unknown);
    if !PushTrigger::confirms(kind, status) {
        debug!(pane_id, ?status, "push confirm: status changed; dropping");
        return;
    }
    let seq = agent
        .get("state_change_seq")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if !trigger.first_send(pane_id, kind, seq) {
        return;
    }
    let list = |method: &str, key: &str| {
        gateway
            .api
            .call(method, json!({}))
            .ok()
            .and_then(|response| result_field(&response, key).cloned())
            .unwrap_or(Value::Null)
    };
    let alert = build_alert(
        kind,
        &agent,
        &list("workspace.list", "workspaces"),
        &list("tab.list", "tabs"),
        &gateway.name,
    );
    let devices = gateway.state().devices.devices.clone();
    let now = Instant::now();
    for device in devices {
        let foreground = gateway.device_foreground_outbounds(&device.device_id);
        if !foreground.is_empty() {
            let message = notify_message(&alert);
            for out in foreground {
                out.send(message.clone());
            }
            continue;
        }
        let (Some(queue), Some(push)) = (&gateway.push_queue, &device.push) else {
            continue;
        };
        let window = limiter.entry(device.device_id.clone()).or_default();
        if !window.try_hit(now, PUSHES_PER_DEVICE_PER_MINUTE, Duration::from_secs(60)) {
            warn!(device_id = %device.device_id, "push rate limit reached; dropping alert");
            continue;
        }
        queue.push(ExpoItem {
            device_id: device.device_id.clone(),
            token: push.expo_token.clone(),
            message: expo_message(&alert, &push.expo_token, &gateway.server_id, &gateway.name),
        });
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExpoItem {
    pub device_id: String,
    pub token: String,
    pub message: Value,
}

/// Bounded queue (drop oldest) feeding the Expo sender thread.
#[derive(Clone)]
pub(crate) struct ExpoQueue {
    inner: Arc<(Mutex<VecDeque<ExpoItem>>, Condvar)>,
}

impl ExpoQueue {
    pub(crate) fn start(access_token: Option<String>, dir: PathBuf) -> Self {
        let queue = Self {
            inner: Arc::new((Mutex::new(VecDeque::new()), Condvar::new())),
        };
        let sender_queue = queue.clone();
        let spawned = crate::thread_spawn::spawn_named("flockr-mobile-expo", move || {
            sender(sender_queue, access_token, dir)
        });
        if let Err(err) = spawned {
            warn!(err = %err, "could not start Expo push sender");
        }
        queue
    }

    pub(crate) fn push(&self, item: ExpoItem) {
        let (lock, ready) = &*self.inner;
        let mut items = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if items.len() >= EXPO_QUEUE {
            items.pop_front();
            warn!("Expo push queue full; dropped the oldest alert");
        }
        items.push_back(item);
        ready.notify_one();
    }

    fn take_batch(&self) -> Vec<ExpoItem> {
        let (lock, ready) = &*self.inner;
        let mut items = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while items.is_empty() {
            items = ready
                .wait(items)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        let count = items.len().min(EXPO_BATCH);
        items.drain(..count).collect()
    }
}

/// What to do with one Expo ticket.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TicketOutcome {
    Ok,
    DeviceNotRegistered,
    Error(String),
}

pub(crate) fn parse_tickets(body: &Value, count: usize) -> Vec<TicketOutcome> {
    let tickets = body.get("data").and_then(Value::as_array);
    (0..count)
        .map(|index| {
            let Some(ticket) = tickets.and_then(|tickets| tickets.get(index)) else {
                return TicketOutcome::Error("missing ticket".to_owned());
            };
            if ticket.get("status").and_then(Value::as_str) == Some("ok") {
                return TicketOutcome::Ok;
            }
            let error = ticket
                .get("details")
                .and_then(|details| details.get("error"))
                .and_then(Value::as_str);
            if error == Some("DeviceNotRegistered") {
                TicketOutcome::DeviceNotRegistered
            } else {
                TicketOutcome::Error(
                    error
                        .or_else(|| ticket.get("message").and_then(Value::as_str))
                        .unwrap_or("unknown error")
                        .to_owned(),
                )
            }
        })
        .collect()
}

enum PostError {
    Retry(String),
    Fatal(String),
}

fn post(agent: &ureq::Agent, token: Option<&str>, body: &str) -> Result<Value, PostError> {
    let mut request = agent
        .post(EXPO_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json");
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request
        .send(body)
        .map_err(|err| PostError::Retry(err.to_string()))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|err| PostError::Retry(err.to_string()))?;
    if status == 429 || status >= 500 {
        return Err(PostError::Retry(format!("HTTP {status}")));
    }
    if !(200..300).contains(&status) {
        return Err(PostError::Fatal(format!("HTTP {status}: {text}")));
    }
    serde_json::from_str(&text).map_err(|err| PostError::Fatal(err.to_string()))
}

fn sender(queue: ExpoQueue, access_token: Option<String>, dir: PathBuf) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    loop {
        let batch = queue.take_batch();
        let body =
            Value::Array(batch.iter().map(|item| item.message.clone()).collect()).to_string();
        let mut attempt = 0;
        let response = loop {
            match post(&agent, access_token.as_deref(), &body) {
                Ok(response) => break Some(response),
                Err(PostError::Fatal(err)) => {
                    warn!(err = %err, "Expo push request rejected");
                    break None;
                }
                Err(PostError::Retry(err)) => {
                    let Some(delay) = EXPO_RETRIES.get(attempt) else {
                        warn!(err = %err, "Expo push failed after retries; dropping");
                        break None;
                    };
                    debug!(err = %err, "Expo push failed; retrying");
                    std::thread::sleep(*delay);
                    attempt += 1;
                }
            }
        };
        let Some(response) = response else {
            continue;
        };
        for (item, outcome) in batch.iter().zip(parse_tickets(&response, batch.len())) {
            match outcome {
                TicketOutcome::Ok => {}
                TicketOutcome::DeviceNotRegistered => {
                    warn!(device_id = %item.device_id, token = %store::redact(&item.token), "Expo token no longer registered; clearing it");
                    let cleared = store::update_devices(&dir, |devices| {
                        if let Some(device) = devices.find_mut(&item.device_id) {
                            if device
                                .push
                                .as_ref()
                                .is_some_and(|push| push.expo_token == item.token)
                            {
                                device.push = None;
                            }
                        }
                    });
                    if let Err(err) = cleared {
                        warn!(err = %err, "could not clear Expo token");
                    }
                }
                TicketOutcome::Error(err) => {
                    warn!(device_id = %item.device_id, token = %store::redact(&item.token), err = %err, "Expo push ticket error");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger() -> PushTrigger {
        PushTrigger::new(true, true, Duration::from_secs(5))
    }

    #[test]
    fn blocked_alerts_wait_for_the_confirm_delay() {
        let mut trigger = trigger();
        let start = Instant::now();
        trigger.on_status("p_1", AgentStatus::Working, start);
        trigger.on_status("p_1", AgentStatus::Blocked, start);
        assert!(trigger.take_due(start + Duration::from_secs(4)).is_empty());
        assert_eq!(
            trigger.take_due(start + Duration::from_secs(5)),
            vec![("p_1".to_owned(), AlertKind::Blocked)]
        );
        assert!(trigger.take_due(start + Duration::from_secs(9)).is_empty());
    }

    #[test]
    fn flicker_cancels_the_pending_check() {
        let mut trigger = trigger();
        let start = Instant::now();
        trigger.on_status("p_1", AgentStatus::Blocked, start);
        trigger.on_status("p_1", AgentStatus::Working, start + Duration::from_secs(1));
        assert!(trigger.take_due(start + Duration::from_secs(10)).is_empty());
    }

    #[test]
    fn repeated_status_does_not_reschedule() {
        let mut trigger = trigger();
        let start = Instant::now();
        trigger.on_status("p_1", AgentStatus::Done, start);
        trigger.on_status("p_1", AgentStatus::Done, start + Duration::from_secs(4));
        assert_eq!(trigger.take_due(start + Duration::from_secs(5)).len(), 1);
    }

    #[test]
    fn a_newer_transition_replaces_the_pending_one() {
        let mut trigger = trigger();
        let start = Instant::now();
        trigger.on_status("p_1", AgentStatus::Blocked, start);
        trigger.on_status("p_1", AgentStatus::Done, start + Duration::from_secs(3));
        assert!(trigger.take_due(start + Duration::from_secs(5)).is_empty());
        assert_eq!(
            trigger.take_due(start + Duration::from_secs(8)),
            vec![("p_1".to_owned(), AlertKind::Done)]
        );
    }

    #[test]
    fn disabled_kinds_never_schedule() {
        let mut trigger = PushTrigger::new(false, true, Duration::ZERO);
        let now = Instant::now();
        trigger.on_status("p_1", AgentStatus::Blocked, now);
        assert!(trigger.take_due(now).is_empty());
        trigger.on_status("p_1", AgentStatus::Done, now);
        assert_eq!(trigger.take_due(now).len(), 1);
    }

    #[test]
    fn dedupe_on_pane_status_and_seq() {
        let mut trigger = trigger();
        assert!(trigger.first_send("p_1", AlertKind::Blocked, 41));
        assert!(!trigger.first_send("p_1", AlertKind::Blocked, 41));
        assert!(trigger.first_send("p_1", AlertKind::Blocked, 42));
        assert!(trigger.first_send("p_1", AlertKind::Done, 41));
        for seq in 0..(DEDUPE_KEYS as u64 + 10) {
            trigger.first_send("p_2", AlertKind::Done, seq);
        }
        assert!(trigger.first_send("p_1", AlertKind::Blocked, 41));
    }

    #[test]
    fn confirm_requires_the_scheduled_status() {
        assert!(PushTrigger::confirms(
            AlertKind::Blocked,
            AgentStatus::Blocked
        ));
        assert!(!PushTrigger::confirms(
            AlertKind::Blocked,
            AgentStatus::Working
        ));
        assert!(!PushTrigger::confirms(AlertKind::Done, AgentStatus::Idle));
    }

    #[test]
    fn alert_text_and_expo_message() {
        let agent = json!({
            "terminal_id": "t_5", "pane_id": "p_3", "workspace_id": "w_1", "tab_id": "tab_2",
            "agent": "claude", "display_agent": "Claude", "agent_status": "blocked",
        });
        let workspaces = json!([{ "workspace_id": "w_1", "label": "flockr" }]);
        let tabs = json!([{ "tab_id": "tab_2", "label": "mobile" }]);
        let alert = build_alert(AlertKind::Blocked, &agent, &workspaces, &tabs, "dk-desktop");
        assert_eq!(alert.title, "Claude is blocked");
        assert_eq!(alert.body, "flockr › mobile · dk-desktop");
        let notify: Value = serde_json::from_str(&notify_message(&alert)).unwrap();
        let expected: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/mobile-v1/server.notify.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(notify, expected);
        let message = expo_message(&alert, "ExponentPushToken[x]", "sid", "dk-desktop");
        assert_eq!(message["interruptionLevel"], "time-sensitive");
        assert_eq!(message["data"]["pane_id"], "p_3");
        let done = build_alert(
            AlertKind::Done,
            &json!({ "name": "builder" }),
            &Value::Null,
            &Value::Null,
            "h",
        );
        assert_eq!(done.title, "builder finished");
        assert_eq!(
            expo_message(&done, "x", "s", "h")["interruptionLevel"],
            "active"
        );
        let unnamed = build_alert(AlertKind::Done, &json!({}), &Value::Null, &Value::Null, "h");
        assert_eq!(unnamed.agent, "Agent");
    }

    #[test]
    fn tickets_map_to_outcomes() {
        let body = json!({ "data": [
            { "status": "ok", "id": "a" },
            { "status": "error", "message": "gone", "details": { "error": "DeviceNotRegistered" } },
            { "status": "error", "message": "too big", "details": { "error": "MessageTooBig" } },
        ]});
        assert_eq!(
            parse_tickets(&body, 4),
            vec![
                TicketOutcome::Ok,
                TicketOutcome::DeviceNotRegistered,
                TicketOutcome::Error("MessageTooBig".to_owned()),
                TicketOutcome::Error("missing ticket".to_owned()),
            ]
        );
    }
}
