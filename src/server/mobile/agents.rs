//! `agents.watch` (§9.3): agent, workspace, and tab lists, re-sent whenever
//! the server's event history moves and the combined result changed.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::api_proxy::result_field;
use super::{Gateway, Outbound};

const POLL: Duration = Duration::from_millis(250);
/// Refresh even without events, in case a change emits none.
const FALLBACK_REFRESH: Duration = Duration::from_secs(5);

/// Fetches `(agents, workspaces, tabs)` through the API. Blocking.
pub(crate) fn fetch_lists(gateway: &Gateway) -> Result<(Value, Value, Value), String> {
    let call = |method: &str, key: &str| {
        gateway
            .api
            .call(method, json!({}))
            .map_err(|err| err.to_string())
            .map(|response| {
                result_field(&response, key)
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new()))
            })
    };
    Ok((
        call("agent.list", "agents")?,
        call("workspace.list", "workspaces")?,
        call("tab.list", "tabs")?,
    ))
}

/// Runs one watch until `stop` fires or the connection goes away.
pub(crate) async fn watch(
    gateway: Arc<Gateway>,
    out: Outbound,
    id: String,
    mut stop: oneshot::Receiver<()>,
) {
    let mut rev = 0_u64;
    let mut last: Option<(Value, Value, Value)> = None;
    let mut last_sequence = None;
    let mut last_refresh = tokio::time::Instant::now();
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = &mut stop => return,
            _ = tick.tick() => {}
        }
        if out.is_closed() {
            return;
        }
        let sequence = gateway.event_hub.current_sequence();
        if last_sequence == Some(sequence) && last_refresh.elapsed() < FALLBACK_REFRESH {
            continue;
        }
        last_sequence = Some(sequence);
        last_refresh = tokio::time::Instant::now();
        let fetch_gateway = gateway.clone();
        let lists = match tokio::task::spawn_blocking(move || fetch_lists(&fetch_gateway)).await {
            Ok(Ok(lists)) => lists,
            Ok(Err(err)) => {
                tracing::debug!(err = %err, "agents.watch refresh failed");
                continue;
            }
            Err(_) => continue,
        };
        if last.as_ref() == Some(&lists) {
            continue;
        }
        rev += 1;
        let message = json!({
            "t": "agents",
            "id": id,
            "rev": rev,
            "agents": lists.0,
            "workspaces": lists.1,
            "tabs": lists.2,
        });
        if !out.send(message.to_string()) {
            return;
        }
        last = Some(lists);
    }
}
