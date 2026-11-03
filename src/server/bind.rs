//! Binding the server's TCP listeners. During a live handoff the new server
//! starts while the old one still holds its ports, so the first retries come
//! quickly and slow down after that.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::warn;

const FIRST_RETRY: Duration = Duration::from_millis(100);
const MAX_RETRY: Duration = Duration::from_secs(10);

/// Binds `addr`, retrying until it works or `shutdown` changes (`None`).
pub(crate) async fn bind_with_retry(
    addr: SocketAddr,
    what: &str,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<TcpListener> {
    let mut delay = FIRST_RETRY;
    loop {
        match TcpListener::bind(addr).await {
            Ok(listener) => return Some(listener),
            Err(err) => {
                // Quiet while the old server is likely still letting go.
                if delay >= Duration::from_secs(1) {
                    warn!(%addr, err = %err, "{what} could not bind; retrying in {} s", delay.as_secs());
                }
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.changed() => return None,
                }
                delay = next_delay(delay);
            }
        }
    }
}

fn next_delay(delay: Duration) -> Duration {
    (delay * 2).min(MAX_RETRY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_start_fast_and_cap_at_ten_seconds() {
        let mut delay = FIRST_RETRY;
        let mut waited = Duration::ZERO;
        while delay < Duration::from_secs(1) {
            waited += delay;
            delay = next_delay(delay);
        }
        assert!(waited < Duration::from_secs(2), "{waited:?}");
        for _ in 0..10 {
            delay = next_delay(delay);
        }
        assert_eq!(delay, MAX_RETRY);
    }

    #[tokio::test]
    async fn binds_once_the_port_is_free() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = held.local_addr().unwrap();
        let (_tx, mut rx) = watch::channel(false);
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            drop(held);
        });
        let started = std::time::Instant::now();
        let listener = bind_with_retry(addr, "test", &mut rx).await;
        assert!(listener.is_some());
        assert!(started.elapsed() < Duration::from_secs(2));
        release.await.unwrap();
    }
}
