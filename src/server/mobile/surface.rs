//! Surface subscriptions (§10): bridge between an in-process headless client
//! connection and one phone subscription.
//!
//! - A drain thread reads frames from the in-process writer and keeps only the
//!   newest one in a `watch` channel, so the server loop never waits on a phone.
//! - A tokio task diffs and encodes JSON, honoring acks (`max_in_flight`) and
//!   `max_fps`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::{debug, warn};

use super::cells::SurfaceEncoder;
use super::protocol::{ErrorCode, SurfaceMode, SurfaceSubscribe};
use super::{Gateway, Outbound, SurfaceOpenError, MAX_COLS, MAX_OUTBOUND_BYTES, MAX_ROWS};
use crate::protocol::{PaneSurfaceFrame, ServerMessage, MAX_FRAME_SIZE};
use crate::server::client_transport::{ClientWriter, InProcessClientItem, ServerEvent};

const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub(crate) enum SubCtl {
    Ack(u64),
    Resync,
    Stop,
}

#[derive(Clone, Default)]
struct Latest {
    frame: Option<Arc<PaneSurfaceFrame>>,
    ended: Option<&'static str>,
}

/// Latest scroll metrics seen for a subscription: `(offset, max)`.
pub(crate) type ScrollState = Arc<Mutex<Option<(u64, u64)>>>;

/// Connection-side handle for one open subscription.
pub(crate) struct SubHandle {
    pub client_id: u64,
    pub pane_id: String,
    pub terminal_id: String,
    pub fit: bool,
    pub ctl: mpsc::UnboundedSender<SubCtl>,
    pub scroll: ScrollState,
    /// Last `pane.scroll` offset this connection asked for, and when.
    pub scroll_target: Option<(u64, std::time::Instant)>,
}

/// Maps a server `ServerShutdown` reason to a `surface.ended` reason.
pub(crate) fn ended_reason(reason: Option<&str>) -> &'static str {
    let reason = reason.unwrap_or_default();
    if reason.contains("taken over") {
        "taken_over"
    } else if reason.contains("exited") || reason.contains("not found") {
        "pane_closed"
    } else if reason.contains("shutting down") || reason.contains("live update") {
        "server_shutdown"
    } else if reason == "detached" {
        "unsubscribed"
    } else {
        "error"
    }
}

fn clamp_size(value: Option<u16>, max: u16) -> u16 {
    value.unwrap_or(max).clamp(2, max)
}

/// Opens a subscription: asks the server loop for a client connection, sends
/// `surface.started`, and spawns the drain thread and encode task.
pub(crate) async fn open(
    gateway: &Arc<Gateway>,
    device_id: &str,
    out: &Outbound,
    ended_tx: mpsc::UnboundedSender<u32>,
    request: &SurfaceSubscribe,
    max_in_flight: u32,
) -> Result<SubHandle, (ErrorCode, String)> {
    let fit = request.mode == SurfaceMode::Fit;
    if fit && (request.cols.is_none() || request.rows.is_none()) {
        return Err((
            ErrorCode::BadRequest,
            "fit mode needs `cols` and `rows`".to_owned(),
        ));
    }
    let cols = clamp_size(request.cols, MAX_COLS);
    let rows = clamp_size(request.rows, MAX_ROWS);
    let client_id = gateway.next_surface_client_id();
    let (writer, reader) = ClientWriter::in_process();
    let (reply_tx, reply_rx) = oneshot::channel();
    gateway
        .server_event_tx
        .send(ServerEvent::MobileSurfaceOpen {
            client_id,
            device_id: device_id.to_owned(),
            target: request.target.clone(),
            fit,
            cols,
            rows,
            writer,
            reply: reply_tx,
        })
        .await
        .map_err(|_| {
            (
                ErrorCode::ServerUnavailable,
                "server loop is gone".to_owned(),
            )
        })?;
    let opened = match tokio::time::timeout(OPEN_TIMEOUT, reply_rx).await {
        Ok(Ok(Ok(opened))) => opened,
        Ok(Ok(Err(SurfaceOpenError::NotFound))) => {
            return Err((
                ErrorCode::NotFound,
                format!("target {} not found", request.target),
            ))
        }
        Ok(Ok(Err(SurfaceOpenError::Busy))) => {
            return Err((
                ErrorCode::PaneBusy,
                "terminal already has an attached client".to_owned(),
            ))
        }
        Ok(Ok(Err(SurfaceOpenError::Unavailable))) | Ok(Err(_)) => {
            return Err((
                ErrorCode::ServerUnavailable,
                "server is not accepting surfaces right now".to_owned(),
            ))
        }
        Err(_) => {
            // The loop may still open it later; make sure it goes away.
            let _ = gateway
                .server_event_tx
                .try_send(ServerEvent::ClientDisconnected { client_id });
            return Err((
                ErrorCode::ServerUnavailable,
                "server did not answer in time".to_owned(),
            ));
        }
    };

    let mut started = json!({
        "t": "surface.started",
        "sub": request.sub,
        "pane_id": opened.pane_id,
        "terminal_id": opened.terminal_id,
        "mode": request.mode.as_str(),
        "cols": opened.cols,
        "rows": opened.rows,
    });
    if let Some(id) = &request.id {
        started["id"] = json!(id);
    }
    out.send(started.to_string());

    let (latest_tx, latest_rx) = watch::channel(Latest::default());
    let server_event_tx = gateway.server_event_tx.clone();
    let spawned = crate::thread_spawn::spawn_named("flockr-mobile-surface", move || {
        drain(reader, latest_tx, server_event_tx, client_id)
    });
    if let Err(err) = spawned {
        warn!(err = %err, "could not start mobile surface drain thread");
        let _ = gateway
            .server_event_tx
            .try_send(ServerEvent::ClientDisconnected { client_id });
        return Err((ErrorCode::Internal, "could not start surface".to_owned()));
    }

    let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
    let scroll: ScrollState = Arc::new(Mutex::new(None));
    tokio::spawn(run(
        gateway.clone(),
        out.clone(),
        ended_tx,
        SubLoop {
            sub: request.sub,
            client_id,
            max_in_flight: u64::from(max_in_flight),
            interval: Duration::from_millis(1000 / u64::from(request.max_fps())),
            scroll: scroll.clone(),
        },
        latest_rx,
        ctl_rx,
    ));
    Ok(SubHandle {
        client_id,
        pane_id: opened.pane_id,
        terminal_id: opened.terminal_id,
        fit,
        ctl: ctl_tx,
        scroll,
        scroll_target: None,
    })
}

fn drain(
    reader: crate::server::client_transport::InProcessClientReader,
    latest: watch::Sender<Latest>,
    server_event_tx: mpsc::Sender<ServerEvent>,
    client_id: u64,
) {
    let mut ended = "error";
    while let Some(item) = reader.recv() {
        let (data, render) = match item {
            InProcessClientItem::Control(data) => (data, false),
            InProcessClientItem::Render(data) => (data, true),
        };
        if render {
            let _ = server_event_tx.blocking_send(ServerEvent::ClientWriterDrained { client_id });
        }
        let message = match crate::protocol::read_message::<_, ServerMessage>(
            &mut data.as_slice(),
            MAX_FRAME_SIZE,
        ) {
            Ok(message) => message,
            Err(err) => {
                debug!(client_id, err = %err, "mobile surface skipped an undecodable frame");
                continue;
            }
        };
        match message {
            ServerMessage::PaneSurface(surface) => {
                let frame = Arc::new(surface);
                latest.send_modify(|latest| latest.frame = Some(frame));
            }
            ServerMessage::ServerShutdown { reason } => {
                ended = ended_reason(reason.as_deref());
                break;
            }
            _ => {}
        }
        if latest.is_closed() {
            ended = "unsubscribed";
            break;
        }
    }
    latest.send_modify(|latest| latest.ended = Some(ended));
}

struct SubLoop {
    sub: u32,
    client_id: u64,
    max_in_flight: u64,
    interval: Duration,
    scroll: ScrollState,
}

async fn run(
    gateway: Arc<Gateway>,
    out: Outbound,
    ended_tx: mpsc::UnboundedSender<u32>,
    params: SubLoop,
    mut latest: watch::Receiver<Latest>,
    mut ctl: mpsc::UnboundedReceiver<SubCtl>,
) {
    let mut encoder = SurfaceEncoder::new(params.sub, true);
    let mut acked = 0_u64;
    let mut dirty = false;
    let mut next_send = Instant::now();
    let reason: &'static str = loop {
        let window_open = encoder.seq().saturating_sub(acked) < params.max_in_flight;
        let send_due = dirty && window_open;
        tokio::select! {
            command = ctl.recv() => match command {
                Some(SubCtl::Ack(seq)) => {
                    if seq <= encoder.seq() {
                        acked = acked.max(seq);
                    }
                }
                Some(SubCtl::Resync) => {
                    encoder.reset();
                    acked = encoder.seq();
                    dirty = true;
                }
                Some(SubCtl::Stop) | None => break "unsubscribed",
            },
            changed = latest.changed() => {
                if changed.is_err() {
                    break latest.borrow().ended.unwrap_or("error");
                }
                if let Some(ended) = latest.borrow().ended {
                    break ended;
                }
                dirty = true;
            },
            _ = tokio::time::sleep_until(next_send), if send_due => {
                dirty = false;
                let frame = latest.borrow_and_update().frame.clone();
                let Some(frame) = frame else { continue };
                if let Some(scroll) = frame.panes.first().and_then(|pane| pane.scroll) {
                    if let Ok(mut state) = params.scroll.lock() {
                        *state = Some((scroll.offset_from_bottom, scroll.max_offset_from_bottom));
                    }
                }
                let Some(encoded) = encoder.encode(&frame) else { continue };
                if encoded.json.len() > MAX_OUTBOUND_BYTES {
                    break "frame_too_large";
                }
                if !out.send(encoded.json) {
                    break "error";
                }
                next_send = Instant::now() + params.interval;
            },
        }
    };
    let _ = gateway
        .server_event_tx
        .send(ServerEvent::ClientDisconnected {
            client_id: params.client_id,
        })
        .await;
    out.send(json!({ "t": "surface.ended", "sub": params.sub, "reason": reason }).to_string());
    let _ = ended_tx.send(params.sub);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_reasons_map_to_ended_reasons() {
        assert_eq!(
            ended_reason(Some("terminal attach taken over")),
            "taken_over"
        );
        assert_eq!(ended_reason(Some("terminal t_5 exited")), "pane_closed");
        assert_eq!(
            ended_reason(Some("terminal attach ended: terminal t_5 not found")),
            "pane_closed"
        );
        assert_eq!(
            ended_reason(Some("server is shutting down")),
            "server_shutdown"
        );
        assert_eq!(
            ended_reason(Some(
                "live update in progress; reconnect after handoff completes"
            )),
            "server_shutdown"
        );
        assert_eq!(ended_reason(Some("detached")), "unsubscribed");
        assert_eq!(ended_reason(None), "error");
    }

    #[test]
    fn subscribe_sizes_are_clamped() {
        assert_eq!(clamp_size(Some(1), MAX_COLS), 2);
        assert_eq!(clamp_size(Some(9999), MAX_COLS), MAX_COLS);
        assert_eq!(clamp_size(None, MAX_ROWS), MAX_ROWS);
    }
}
