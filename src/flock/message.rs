//! Messages between agents, on any machine in the flock.
//!
//! A message goes from the sender's flock node to the recipient's, through
//! the hub when they are different machines. The sender's node stamps who
//! sent it; the recipient's node checks the stamp against the member the hub
//! says the stream came from, applies the rules and limits, and keeps the
//! message in the recipient's inbox until the agent reads it.
//!
//! Files under `state_dir()/flock/`, written only by the server:
//! - `inbox/<terminal>/<id>.json`: received messages per recipient terminal.
//! - `pending/<id>.json`: messages waiting for a person to approve them.
//! - `sent/<id>.json`: messages this machine's agents sent, until they
//!   expire, so the replies to them need no approval.
//! - `message-rules.json`: sender and recipient pairs a person always allows.
//! - `messages.jsonl`: the audit log of every message event.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::ApproveMessages;
use crate::mobile::store::{atomic_write, ensure_dir};

pub(crate) const MAX_HOPS: u32 = 8;
pub(crate) const TTL_SECS: u64 = 24 * 3600;
pub(crate) const MAX_BODY_CHARS: usize = 16 * 1024;
/// Unread messages one recipient may hold.
pub(crate) const MAX_QUEUED: usize = 50;
/// Messages one sender may send one recipient machine per minute.
pub(crate) const RATE_PER_MINUTE: usize = 20;
const AUDIT_FILE: &str = "messages.jsonl";
const AUDIT_MAX_BYTES: u64 = 8 * 1024 * 1024;
const RULES_FILE: &str = "message-rules.json";
const INBOX_DIR: &str = "inbox";
const PENDING_DIR: &str = "pending";
const SENT_DIR: &str = "sent";
pub(crate) const SOCKET_FILE: &str = "flock.sock";

pub(crate) fn socket_path() -> PathBuf {
    super::store::flock_dir().join(SOCKET_FILE)
}

/// One end of a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Party {
    /// Member ID of the machine.
    pub member: String,
    /// The machine's name in the flock.
    pub machine: String,
    /// The agent's name, or its pane ID when it has none. `None` is a person
    /// typing in a shell or outside any pane.
    #[serde(default)]
    pub agent: Option<String>,
    /// Terminal ID on that machine, once known.
    #[serde(default)]
    pub terminal: Option<String>,
    /// The git remote of the agent's working folder, normalized.
    #[serde(default)]
    pub repo: Option<String>,
}

impl Party {
    pub(crate) fn label(&self) -> String {
        match &self.agent {
            Some(agent) => format!("{}:{agent}", self.machine),
            None => format!("{} (a person)", self.machine),
        }
    }

    /// The same agent on the same machine.
    pub(crate) fn same_agent(&self, other: &Party) -> bool {
        self.member == other.member && self.agent.is_some() && self.agent == other.agent
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Message {
    pub id: String,
    pub conversation: String,
    #[serde(default)]
    pub reply_to: Option<String>,
    pub hops: u32,
    /// The sender is waiting for a reply.
    #[serde(default)]
    pub wait: bool,
    pub sent_at: u64,
    pub expires_at: u64,
    pub from: Party,
    pub to: Party,
    pub body: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InboxState {
    Queued,
    Notified,
    Read,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InboxEntry {
    pub message: Message,
    pub state: InboxState,
    #[serde(default)]
    pub notified_at: Option<u64>,
}

/// Why a message was not accepted. The text goes back to the sender.
pub(crate) fn check_limits(message: &Message, now: u64) -> Result<(), String> {
    if message.expires_at <= now {
        return Err("the message expired".into());
    }
    if message.hops > MAX_HOPS {
        return Err(format!(
            "the conversation passed {MAX_HOPS} replies; a person has to restart it"
        ));
    }
    if message.body.chars().count() > MAX_BODY_CHARS {
        return Err(format!(
            "the message is longer than {MAX_BODY_CHARS} characters"
        ));
    }
    if message.body.trim().is_empty() {
        return Err("the message is empty".into());
    }
    Ok(())
}

/// `git@github.com:org/repo.git`, `https://github.com/org/repo` and
/// `ssh://git@github.com/org/repo/` all become `github.com/org/repo`.
pub(crate) fn normalize_repo(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let without_user = without_scheme
        .split_once('@')
        .filter(|(user, _)| !user.contains('/'))
        .map_or(without_scheme, |(_, rest)| rest);
    // scp-style `host:path`; a `host:port/path` URL had a scheme.
    let joined = if url.contains("://") {
        without_user.to_owned()
    } else {
        without_user.replacen(':', "/", 1)
    };
    let trimmed = joined.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let (host, path) = trimmed.split_once('/').unwrap_or((trimmed, ""));
    Some(format!("{}/{path}", host.to_ascii_lowercase()))
}

fn inbox_dir(dir: &Path, terminal: &str) -> PathBuf {
    dir.join(INBOX_DIR).join(terminal)
}

/// IDs come from the network; keep them to safe file names.
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn read_entry(path: &Path) -> Option<InboxEntry> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub(crate) fn save_entry(dir: &Path, terminal: &str, entry: &InboxEntry) -> io::Result<()> {
    if !valid_id(terminal) || !valid_id(&entry.message.id) {
        return Err(io::Error::other("invalid message or terminal ID"));
    }
    let bytes = serde_json::to_vec_pretty(entry).map_err(io::Error::other)?;
    atomic_write(
        &inbox_dir(dir, terminal).join(format!("{}.json", entry.message.id)),
        &bytes,
    )
}

/// Every inbox entry for `terminal`, oldest first.
pub(crate) fn inbox(dir: &Path, terminal: &str) -> Vec<InboxEntry> {
    if !valid_id(terminal) {
        return Vec::new();
    }
    let mut entries: Vec<InboxEntry> = fs::read_dir(inbox_dir(dir, terminal))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|item| read_entry(&item.path()))
        .collect();
    entries.sort_by(|a, b| {
        (a.message.sent_at, &a.message.id).cmp(&(b.message.sent_at, &b.message.id))
    });
    entries
}

/// Terminals with an inbox.
pub(crate) fn inbox_terminals(dir: &Path) -> Vec<String> {
    fs::read_dir(dir.join(INBOX_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|item| item.file_name().into_string().ok())
        .filter(|name| valid_id(name))
        .collect()
}

/// A received message by ID, in any inbox.
pub(crate) fn find_received(dir: &Path, id: &str) -> Option<InboxEntry> {
    if !valid_id(id) {
        return None;
    }
    inbox_terminals(dir)
        .into_iter()
        .find_map(|terminal| read_entry(&inbox_dir(dir, &terminal).join(format!("{id}.json"))))
}

pub(crate) fn remove_entry(dir: &Path, terminal: &str, id: &str) {
    if valid_id(terminal) && valid_id(id) {
        let _ = fs::remove_file(inbox_dir(dir, terminal).join(format!("{id}.json")));
    }
}

pub(crate) fn save_pending(dir: &Path, message: &Message) -> io::Result<()> {
    if !valid_id(&message.id) {
        return Err(io::Error::other("invalid message ID"));
    }
    let bytes = serde_json::to_vec_pretty(message).map_err(io::Error::other)?;
    atomic_write(
        &dir.join(PENDING_DIR).join(format!("{}.json", message.id)),
        &bytes,
    )
}

pub(crate) fn pending(dir: &Path) -> Vec<Message> {
    let mut messages: Vec<Message> = fs::read_dir(dir.join(PENDING_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|item| serde_json::from_slice(&fs::read(item.path()).ok()?).ok())
        .collect();
    messages.sort_by_key(|message| message.sent_at);
    messages
}

pub(crate) fn take_pending(dir: &Path, id: &str) -> Option<Message> {
    if !valid_id(id) {
        return None;
    }
    let path = dir.join(PENDING_DIR).join(format!("{id}.json"));
    let message = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    fs::remove_file(&path).ok()?;
    Some(message)
}

/// Keeps a message this machine sent, and drops sent messages that expired.
pub(crate) fn save_sent(dir: &Path, message: &Message, now: u64) -> io::Result<()> {
    if !valid_id(&message.id) {
        return Err(io::Error::other("invalid message ID"));
    }
    for item in fs::read_dir(dir.join(SENT_DIR))
        .into_iter()
        .flatten()
        .flatten()
    {
        let expired = fs::read(item.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Message>(&bytes).ok())
            .is_none_or(|sent| sent.expires_at <= now);
        if expired {
            let _ = fs::remove_file(item.path());
        }
    }
    let bytes = serde_json::to_vec_pretty(message).map_err(io::Error::other)?;
    atomic_write(
        &dir.join(SENT_DIR).join(format!("{}.json", message.id)),
        &bytes,
    )
}

/// Whether `message` replies to one this machine sent: from the agent it went
/// to, to the agent that sent it, in the same conversation.
fn answers_sent(dir: &Path, message: &Message) -> bool {
    let Some(id) = message.reply_to.as_deref().filter(|id| valid_id(id)) else {
        return false;
    };
    let Some(sent) = fs::read(dir.join(SENT_DIR).join(format!("{id}.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Message>(&bytes).ok())
    else {
        return false;
    };
    sent.conversation == message.conversation
        && sent.to.same_agent(&message.from)
        && sent.from.same_agent(&message.to)
}

/// A sender and recipient pair a person always allows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Rule {
    pub from_member: String,
    pub from_agent: Option<String>,
    pub to_agent: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RulesFile {
    version: u32,
    allow: Vec<Rule>,
}

fn load_rules(dir: &Path) -> RulesFile {
    fs::read(dir.join(RULES_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn rule_for(message: &Message) -> Option<Rule> {
    Some(Rule {
        from_member: message.from.member.clone(),
        from_agent: message.from.agent.clone(),
        to_agent: message.to.agent.clone()?,
    })
}

pub(crate) fn add_rule(dir: &Path, rule: Rule) -> io::Result<()> {
    let mut rules = load_rules(dir);
    if !rules.allow.contains(&rule) {
        rules.allow.push(rule);
    }
    rules.version = 1;
    let bytes = serde_json::to_vec_pretty(&rules).map_err(io::Error::other)?;
    atomic_write(&dir.join(RULES_FILE), &bytes)
}

/// A person may always message an agent, and agents in the same repo may
/// always message each other; anything else needs a saved rule. The rules
/// keep agents from messaging across projects in normal use; any process
/// running as the user can still claim to be a person.
/// Whether `message` goes through without a person's approval under `mode`.
/// A person's own messages always do.
pub(crate) fn allowed(dir: &Path, message: &Message, mode: ApproveMessages) -> bool {
    if message.from.agent.is_none() || mode == ApproveMessages::Never {
        return true;
    }
    if mode == ApproveMessages::OtherProjects {
        if message.from.repo.is_some() && message.from.repo == message.to.repo {
            return true;
        }
        if answers_sent(dir, message) {
            return true;
        }
    }
    rule_for(message).is_some_and(|rule| load_rules(dir).allow.contains(&rule))
}

/// Appends one event to the audit log. The body is shortened.
pub(crate) fn audit(dir: &Path, event: &str, message: &Message, detail: Option<&str>) {
    let mut body: String = message.body.chars().take(200).collect();
    if message.body.chars().count() > 200 {
        body.push('…');
    }
    let line = json!({
        "at": crate::mobile::store::now_unix(),
        "event": event,
        "id": message.id,
        "conversation": message.conversation,
        "reply_to": message.reply_to,
        "from": message.from.label(),
        "to": message.to.label(),
        "body": body,
        "detail": detail,
    });
    if let Err(err) = append_audit(dir, &line) {
        tracing::warn!(err = %err, "could not write the flock message log");
    }
}

fn append_audit(dir: &Path, line: &Value) -> io::Result<()> {
    ensure_dir(dir)?;
    let path = dir.join(AUDIT_FILE);
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > AUDIT_MAX_BYTES) {
        fs::rename(&path, dir.join(format!("{AUDIT_FILE}.1")))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    crate::ipc::restrict_socket_permissions(&path, 0o600)?;
    writeln!(file, "{line}")
}

/// The last `limit` audit events, oldest first.
pub(crate) fn audit_tail(dir: &Path, limit: usize) -> Vec<Value> {
    let text = fs::read_to_string(dir.join(AUDIT_FILE)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(limit)..]
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn party(member: &str, agent: Option<&str>, repo: Option<&str>) -> Party {
        Party {
            member: member.into(),
            machine: member.trim_start_matches("m_").into(),
            agent: agent.map(Into::into),
            terminal: agent.map(|agent| format!("term_{agent}")),
            repo: repo.map(Into::into),
        }
    }

    fn allows(dir: &Path, message: &Message) -> bool {
        allowed(dir, message, ApproveMessages::OtherProjects)
    }

    pub(crate) fn message(id: &str, from: Party, to: Party) -> Message {
        Message {
            id: id.into(),
            conversation: format!("c_{id}"),
            reply_to: None,
            hops: 0,
            wait: false,
            sent_at: 100,
            expires_at: 100 + TTL_SECS,
            from,
            to,
            body: "hello".into(),
        }
    }

    #[test]
    fn repo_urls_normalize_to_host_and_path() {
        for url in [
            "git@github.com:org/repo.git",
            "https://github.com/org/repo",
            "ssh://git@GitHub.com/org/repo/",
            "https://user@github.com/org/repo.git",
        ] {
            assert_eq!(
                normalize_repo(url).as_deref(),
                Some("github.com/org/repo"),
                "{url}"
            );
        }
        assert_eq!(normalize_repo("  "), None);
        assert_ne!(
            normalize_repo("git@github.com:org/one.git"),
            normalize_repo("git@github.com:org/two.git")
        );
    }

    #[test]
    fn limits_refuse_expired_long_empty_and_looping_messages() {
        let mut msg = message(
            "m1",
            party("m_a", Some("x"), None),
            party("m_b", Some("y"), None),
        );
        assert!(check_limits(&msg, 100).is_ok());
        assert!(check_limits(&msg, 100 + TTL_SECS).is_err());
        msg.hops = MAX_HOPS + 1;
        assert!(check_limits(&msg, 100).is_err());
        msg.hops = 0;
        msg.body = " ".into();
        assert!(check_limits(&msg, 100).is_err());
        msg.body = "x".repeat(MAX_BODY_CHARS + 1);
        assert!(check_limits(&msg, 100).is_err());
    }

    #[test]
    fn same_repo_or_a_saved_rule_allows_a_message() {
        let dir = crate::mobile::store::tests::temp_dir("flock-message-rules");
        let repo = Some("github.com/org/repo");
        let same = message(
            "m1",
            party("m_a", Some("x"), repo),
            party("m_b", Some("y"), repo),
        );
        assert!(allows(&dir, &same));
        let other = message(
            "m2",
            party("m_a", Some("x"), Some("github.com/org/other")),
            party("m_b", Some("y"), repo),
        );
        assert!(!allows(&dir, &other));
        add_rule(&dir, rule_for(&other).unwrap()).unwrap();
        add_rule(&dir, rule_for(&other).unwrap()).unwrap();
        assert!(allows(&dir, &other));
        assert_eq!(load_rules(&dir).allow.len(), 1);
        let unrepoed = message(
            "m3",
            party("m_a", Some("z"), None),
            party("m_b", Some("y"), None),
        );
        let person = message(
            "m4",
            party("m_a", None, None),
            party("m_b", Some("y"), None),
        );
        assert!(allows(&dir, &person));
        assert!(
            !allows(&dir, &unrepoed),
            "no repo on either side is not a match"
        );
    }

    #[test]
    fn a_reply_to_a_sent_message_needs_no_approval() {
        let dir = crate::mobile::store::tests::temp_dir("flock-message-replies");
        let sent = message(
            "m1",
            party("m_a", Some("x"), Some("github.com/org/one")),
            party("m_b", Some("y"), Some("github.com/org/two")),
        );
        save_sent(&dir, &sent, 100).unwrap();
        let reply = |id: &str, from: Party, to: Party| Message {
            reply_to: Some("m1".into()),
            conversation: sent.conversation.clone(),
            ..message(id, from, to)
        };
        assert!(allows(
            &dir,
            &reply("r1", sent.to.clone(), sent.from.clone())
        ));
        let stranger = party("m_b", Some("z"), None);
        assert!(
            !allows(&dir, &reply("r2", stranger, sent.from.clone())),
            "only the agent it went to may answer"
        );
        let elsewhere = party("m_a", Some("w"), None);
        assert!(
            !allows(&dir, &reply("r3", sent.to.clone(), elsewhere)),
            "the answer goes to the agent that sent it"
        );
        let unsent = Message {
            reply_to: Some("m9".into()),
            ..reply("r4", sent.to.clone(), sent.from.clone())
        };
        assert!(!allows(&dir, &unsent));

        // Expired sent messages are dropped when the next one is saved.
        let later = message("m2", sent.from.clone(), sent.to.clone());
        save_sent(&dir, &later, sent.expires_at).unwrap();
        assert!(!allows(
            &dir,
            &reply("r5", sent.to.clone(), sent.from.clone())
        ));
    }

    #[test]
    fn approve_messages_never_and_always_change_what_waits() {
        let dir = crate::mobile::store::tests::temp_dir("flock-message-modes");
        let repo = Some("github.com/org/repo");
        let same = message(
            "m1",
            party("m_a", Some("x"), repo),
            party("m_b", Some("y"), repo),
        );
        let other = message(
            "m2",
            party("m_a", Some("x"), Some("github.com/org/other")),
            party("m_b", Some("y"), repo),
        );
        let person = message(
            "m3",
            party("m_a", None, None),
            party("m_b", Some("y"), repo),
        );
        assert!(allowed(&dir, &other, ApproveMessages::Never));
        assert!(!allowed(&dir, &same, ApproveMessages::Always));
        assert!(allowed(&dir, &person, ApproveMessages::Always));
        add_rule(&dir, rule_for(&same).unwrap()).unwrap();
        assert!(
            allowed(&dir, &same, ApproveMessages::Always),
            "a pair a person allowed still goes through"
        );
    }

    #[test]
    fn inbox_and_pending_round_trip_and_audit_appends() {
        let dir = crate::mobile::store::tests::temp_dir("flock-message-inbox");
        let msg = message(
            "m1",
            party("m_a", Some("x"), None),
            party("m_b", Some("y"), None),
        );
        let entry = InboxEntry {
            message: msg.clone(),
            state: InboxState::Queued,
            notified_at: None,
        };
        save_entry(&dir, "term_y", &entry).unwrap();
        assert!(save_entry(&dir, "../x", &entry).is_err());
        assert_eq!(inbox(&dir, "term_y"), vec![entry]);
        assert_eq!(inbox_terminals(&dir), vec!["term_y".to_owned()]);
        assert_eq!(find_received(&dir, "m1").unwrap().message, msg);
        remove_entry(&dir, "term_y", "m1");
        assert!(inbox(&dir, "term_y").is_empty());

        save_pending(&dir, &msg).unwrap();
        assert_eq!(pending(&dir), vec![msg.clone()]);
        assert_eq!(take_pending(&dir, "m1"), Some(msg.clone()));
        assert!(pending(&dir).is_empty());

        audit(&dir, "sent", &msg, None);
        audit(&dir, "read", &msg, Some("by y"));
        let tail = audit_tail(&dir, 1);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0]["event"], "read");
        assert_eq!(tail[0]["from"], "a:x");
    }
}
