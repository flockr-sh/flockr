//! Serving another member's API stream from this server's own API socket.
//!
//! The hub stamps who opened the stream and that member's role; each request
//! is checked against the role before it reaches the socket.

use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::debug;

use crate::flock::access::RequestFilter;
use crate::flock::client::{data_message, error_line};
use crate::flock::pipe;
use crate::flock::store::Role;

/// Starts serving stream `stream` for `from`. Returns where to send the
/// caller's bytes; dropping it closes the stream. Answers go to `out` as
/// `data` and finally `close` messages for `stream`.
pub(super) fn serve_stream(
    stream: u64,
    from: String,
    role: Role,
    out: mpsc::UnboundedSender<Value>,
) -> mpsc::UnboundedSender<Vec<u8>> {
    let (input_tx, input) = mpsc::unbounded_channel();
    tokio::spawn(run(stream, from, role, input, out));
    input_tx
}

async fn run(
    stream: u64,
    from: String,
    role: Role,
    mut input: mpsc::UnboundedReceiver<Vec<u8>>,
    out: mpsc::UnboundedSender<Value>,
) {
    let socket = crate::api::socket_path();
    let connected = tokio::task::spawn_blocking(move || {
        crate::ipc::connect_local_stream(&socket).and_then(pipe::start)
    })
    .await;
    let (writer, mut answers) = match connected {
        Ok(Ok(piped)) => piped,
        Ok(Err(err)) => {
            let reason = format!("this machine's API is unavailable: {err}");
            let _ = out.send(json!({ "t": "close", "s": stream, "error": reason }));
            return;
        }
        Err(_) => return,
    };
    debug!(%from, stream, role = role.as_str(), "flock API stream opened");
    let mut filter = RequestFilter::new(role);
    loop {
        tokio::select! {
            bytes = input.recv() => {
                let Some(bytes) = bytes else {
                    return;
                };
                let Some(filtered) = filter.feed(&bytes) else {
                    let _ = out.send(data_message(stream, &error_line("too_large", "request line too long")));
                    let _ = out.send(json!({ "t": "close", "s": stream }));
                    return;
                };
                if !filtered.refused.is_empty() {
                    let _ = out.send(data_message(stream, &filtered.refused));
                }
                if !filtered.forward.is_empty() && !writer.write(filtered.forward) {
                    let _ = out.send(json!({ "t": "close", "s": stream }));
                    return;
                }
            }
            answer = answers.recv() => match answer {
                Some(bytes) => {
                    if out.send(data_message(stream, &bytes)).is_err() {
                        return;
                    }
                }
                None => {
                    let _ = out.send(json!({ "t": "close", "s": stream }));
                    return;
                }
            },
        }
    }
}
