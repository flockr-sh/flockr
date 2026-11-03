//! What each role may do through a flock API stream. The hub stamps the
//! caller's role on every stream it relays; the machine serving the stream
//! checks each request against it.

use serde_json::{json, Value};

use super::store::Role;

/// Methods a viewer may call: reads, waits and event streams.
const VIEWER_METHODS: &[&str] = &[
    "ping",
    "session.snapshot",
    "workspace.list",
    "workspace.get",
    "tab.list",
    "tab.get",
    "agent.list",
    "agent.get",
    "agent.read",
    "agent.explain",
    "agent.wait",
    "pane.list",
    "pane.get",
    "pane.current",
    "pane.layout",
    "pane.read",
    "pane.link.list",
    "pane.process_info",
    "pane.wait_for_output",
    "worktree.list",
    "events.subscribe",
    "events.wait",
];

pub(crate) fn allows(role: Role, method: &str) -> bool {
    match role {
        Role::Admin | Role::Member => true,
        Role::Viewer => VIEWER_METHODS.contains(&method),
    }
}

/// Splits a client's bytes into request lines and checks each one. Lines
/// pass through unchanged; a refused request gets an API error instead.
pub(crate) struct RequestFilter {
    role: Role,
    partial: Vec<u8>,
}

/// The result of feeding bytes to a [`RequestFilter`].
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Filtered {
    /// Bytes to pass to the API socket.
    pub forward: Vec<u8>,
    /// Error lines to send back to the caller.
    pub refused: Vec<u8>,
}

/// Longest request line the filter buffers before giving up on it.
const MAX_LINE: usize = 16 * 1024 * 1024;

impl RequestFilter {
    pub(crate) fn new(role: Role) -> Self {
        Self {
            role,
            partial: Vec::new(),
        }
    }

    /// `None` when a line grows past the limit; the stream should close.
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Option<Filtered> {
        let mut out = Filtered::default();
        if self.role != Role::Viewer {
            out.forward = bytes.to_vec();
            return Some(out);
        }
        self.partial.extend_from_slice(bytes);
        while let Some(end) = self.partial.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            let request: Value = serde_json::from_slice(&line).unwrap_or(Value::Null);
            let method = request["method"].as_str().unwrap_or_default();
            if allows(self.role, method) {
                out.forward.extend_from_slice(&line);
            } else {
                let error = json!({
                    "id": request["id"].as_str().unwrap_or_default(),
                    "error": {
                        "code": "forbidden",
                        "message": format!("a {} may not call {method}", self.role.as_str()),
                    },
                });
                out.refused.extend_from_slice(error.to_string().as_bytes());
                out.refused.push(b'\n');
            }
        }
        (self.partial.len() <= MAX_LINE).then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewers_read_but_cannot_type_or_start_agents() {
        assert!(allows(Role::Admin, "pane.send_text"));
        assert!(allows(Role::Member, "agent.start"));
        assert!(allows(Role::Viewer, "agent.list"));
        assert!(!allows(Role::Viewer, "pane.send_text"));
        assert!(!allows(Role::Viewer, "server.stop"));

        let mut filter = RequestFilter::new(Role::Viewer);
        let list = b"{\"id\":\"1\",\"method\":\"agent.list\",\"params\":{}}\n";
        let prompt = b"{\"id\":\"2\",\"method\":\"agent.prompt\",\"params\":{}}\n";
        // A line split across reads is held until it ends.
        assert_eq!(filter.feed(&list[..10]).unwrap(), Filtered::default());
        let out = filter.feed(&[&list[10..], &prompt[..]].concat()).unwrap();
        assert_eq!(out.forward, list.to_vec());
        let refused: Value = serde_json::from_slice(&out.refused).unwrap();
        assert_eq!(refused["id"], "2");
        assert_eq!(refused["error"]["code"], "forbidden");

        let mut admin = RequestFilter::new(Role::Admin);
        assert_eq!(admin.feed(prompt).unwrap().forward, prompt.to_vec());
    }
}
