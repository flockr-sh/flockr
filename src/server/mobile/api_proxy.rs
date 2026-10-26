//! `api.request` passthrough to the local JSON API socket (§9.2).
//!
//! Each request opens a fresh API connection, the same way the CLI does, so
//! the API server, `events.subscribe`, and the wait methods are reused
//! unchanged. Socket I/O is blocking and runs on `spawn_blocking` threads.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::protocol::{self, ErrorCode};
use super::{Outbound, MAX_OUTBOUND_BYTES};
use crate::ipc::{LocalStream, LocalStreamReadCount};

/// Methods the phone may call. Anything else answers `method_not_allowed`.
pub(crate) const ALLOWED_METHODS: &[&str] = &[
    "ping",
    "session.snapshot",
    "workspace.list",
    "workspace.get",
    "workspace.focus",
    "workspace.create",
    "workspace.rename",
    "workspace.close",
    "tab.list",
    "tab.get",
    "tab.focus",
    "tab.create",
    "tab.rename",
    "tab.close",
    "agent.list",
    "agent.get",
    "agent.read",
    "agent.explain",
    "agent.send_keys",
    "agent.rename",
    "agent.focus",
    "agent.start",
    "agent.prompt",
    "agent.wait",
    "pane.list",
    "pane.get",
    "pane.current",
    "pane.layout",
    "pane.read",
    "pane.focus",
    "pane.split",
    "pane.close",
    "pane.zoom",
    "pane.send_text",
    "pane.send_keys",
    "pane.send_input",
    "pane.scroll",
    "pane.resize",
    "pane.rename",
    "pane.mark_unread",
    "pane.mark_read",
    "pane.link.list",
    "pane.process_info",
    "worktree.list",
    "worktree.create",
    "worktree.open",
    "events.subscribe",
    "events.wait",
    "pane.wait_for_output",
    "notification.show",
];

pub(crate) fn method_allowed(method: &str) -> bool {
    ALLOWED_METHODS.contains(&method)
}

/// Streaming methods keep the API connection open after the first line.
fn is_stream(method: &str) -> bool {
    method == "events.subscribe"
}

/// Failure of a one-shot internal API call.
#[derive(Debug)]
pub(crate) enum ApiCallError {
    /// The server could not be reached or answered garbage.
    Unavailable(String),
    /// The API answered with `{"error":{code,message}}`.
    Api { code: String, message: String },
}

impl std::fmt::Display for ApiCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(message) => write!(f, "{message}"),
            Self::Api { code, message } => write!(f, "{code}: {message}"),
        }
    }
}

/// Talks to this server's own API socket.
#[derive(Clone, Debug)]
pub(crate) struct ApiCaller {
    socket: PathBuf,
}

const READ_POLL_MIN: Duration = Duration::from_millis(2);
const READ_POLL_MAX: Duration = Duration::from_millis(50);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

impl ApiCaller {
    pub(crate) fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    fn connect_and_send(&self, id: &str, method: &str, params: &Value) -> io::Result<LocalStream> {
        let mut stream = crate::ipc::connect_local_stream(&self.socket)?;
        let mut line = serde_json::to_vec(&json!({ "id": id, "method": method, "params": params }))
            .map_err(io::Error::other)?;
        line.push(b'\n');
        stream.write_all(&line)?;
        stream.flush()?;
        crate::ipc::set_local_stream_polling(&mut stream, true)?;
        Ok(stream)
    }

    /// One request, one response line. Blocking; returns the full response
    /// object (`{"id","result"}`).
    pub(crate) fn call(&self, method: &str, params: Value) -> Result<Value, ApiCallError> {
        let mut stream = self
            .connect_and_send("mobile", method, &params)
            .map_err(|err| ApiCallError::Unavailable(err.to_string()))?;
        let cancel = AtomicBool::new(false);
        let deadline = std::time::Instant::now() + CALL_TIMEOUT;
        let mut reader = LineReader::default();
        let line = reader
            .next_line(&mut stream, &cancel, Some(deadline), MAX_OUTBOUND_BYTES)
            .map_err(|err| ApiCallError::Unavailable(err.to_string()))?
            .ok_or_else(|| ApiCallError::Unavailable("api closed without a response".into()))?;
        let value: Value = serde_json::from_slice(&line)
            .map_err(|err| ApiCallError::Unavailable(err.to_string()))?;
        if let Some(error) = value.get("error") {
            return Err(ApiCallError::Api {
                code: error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("error")
                    .to_owned(),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            });
        }
        Ok(value)
    }

    /// Runs one `api.request` for the phone and writes `api.response`,
    /// `api.event`, and `api.end` to `out`. Blocking.
    pub(crate) fn proxy(
        &self,
        id: &str,
        method: &str,
        params: &Value,
        out: &Outbound,
        cancel: &Arc<AtomicBool>,
    ) {
        let mut stream = match self.connect_and_send(id, method, params) {
            Ok(stream) => stream,
            Err(err) => {
                out.send(protocol::error(
                    Some(id),
                    ErrorCode::ServerUnavailable,
                    format!("flockr API unavailable: {err}"),
                ));
                return;
            }
        };
        let streaming = is_stream(method);
        let mut reader = LineReader::default();
        let mut first = true;
        loop {
            let line = match reader.next_line(&mut stream, cancel, None, MAX_OUTBOUND_BYTES) {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(err) if err.kind() == io::ErrorKind::InvalidData => {
                    out.send(protocol::error(
                        Some(id),
                        ErrorCode::PayloadTooLarge,
                        "API reply exceeds 4 MiB",
                    ));
                    return;
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {
                    // `api.cancel`: closing the stream ends the request.
                    out.send(protocol::reply("api.end", id));
                    return;
                }
                Err(err) => {
                    if first {
                        out.send(protocol::error(
                            Some(id),
                            ErrorCode::ServerUnavailable,
                            format!("flockr API read failed: {err}"),
                        ));
                        return;
                    }
                    break;
                }
            };
            let value: Value = match serde_json::from_slice(&line) {
                Ok(value) => value,
                Err(err) => {
                    out.send(protocol::error(
                        Some(id),
                        ErrorCode::Internal,
                        format!("API sent invalid JSON: {err}"),
                    ));
                    return;
                }
            };
            let message = if first {
                json!({ "t": "api.response", "id": id, "response": value })
            } else {
                json!({ "t": "api.event", "id": id, "event": value })
            };
            if !out.send(message.to_string()) {
                return;
            }
            first = false;
            if !streaming {
                return;
            }
        }
        if streaming && !first {
            out.send(protocol::reply("api.end", id));
        } else if first {
            out.send(protocol::error(
                Some(id),
                ErrorCode::ServerUnavailable,
                "flockr API closed without a response",
            ));
        }
    }
}

/// Newline-delimited reader over a polling local stream that honors a cancel
/// flag and an optional deadline.
#[derive(Default)]
struct LineReader {
    buffer: Vec<u8>,
    closed: bool,
}

impl LineReader {
    fn next_line(
        &mut self,
        stream: &mut LocalStream,
        cancel: &AtomicBool,
        deadline: Option<std::time::Instant>,
        max: usize,
    ) -> io::Result<Option<Vec<u8>>> {
        let mut chunk = [0_u8; 16 * 1024];
        let mut wait = READ_POLL_MIN;
        loop {
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let mut line = self.buffer.drain(..=end).collect::<Vec<_>>();
                line.pop();
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                return Ok(Some(line));
            }
            if self.buffer.len() > max {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
            }
            if self.closed {
                return Ok(None);
            }
            if cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "canceled"));
            }
            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "API call timed out",
                ));
            }
            match crate::ipc::poll_local_stream_read_count(stream, &mut chunk)? {
                LocalStreamReadCount::Data(count) => {
                    self.buffer.extend_from_slice(&chunk[..count]);
                    wait = READ_POLL_MIN;
                }
                LocalStreamReadCount::Closed => self.closed = true,
                LocalStreamReadCount::Pending => {
                    std::thread::sleep(wait);
                    wait = (wait * 2).min(READ_POLL_MAX);
                }
            }
        }
    }
}

/// Result arrays of a `{"result": {...}}` response, by key.
pub(crate) fn result_field<'a>(response: &'a Value, key: &str) -> Option<&'a Value> {
    response.get("result").and_then(|result| result.get(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_blocks_lifecycle_and_install_methods() {
        for allowed in [
            "agent.list",
            "pane.send_keys",
            "pane.mark_read",
            "events.subscribe",
            "ping",
        ] {
            assert!(method_allowed(allowed), "{allowed}");
        }
        for blocked in [
            "server.stop",
            "server.reload_agent_manifests",
            "plugin.install",
            "integration.install",
            "command.invoke",
            "client.window_title.set",
            "pane.edit_scrollback",
            "worktree.remove",
            "layout.apply",
            "",
        ] {
            assert!(!method_allowed(blocked), "{blocked}");
        }
    }

    #[test]
    fn allowlist_names_real_api_methods() {
        let schema: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("docs/next/api/flockr-api.schema.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let text = schema.to_string();
        for method in ALLOWED_METHODS {
            assert!(
                text.contains(&format!("\"{method}\"")),
                "{method} is not an API method"
            );
        }
    }
}
