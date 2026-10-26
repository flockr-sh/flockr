//! Binds one configured address (retrying every 10 s) and accepts connections.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tracing::{debug, info, warn};

use super::{connection, Gateway, BIND_RETRY};

pub(crate) async fn run(gateway: Arc<Gateway>, addr: SocketAddr) {
    let mut shutdown = gateway.shutdown.clone();
    let listener = loop {
        match TcpListener::bind(addr).await {
            Ok(listener) => break listener,
            Err(err) => {
                warn!(%addr, err = %err, "mobile gateway could not bind; retrying in 10 s");
                tokio::select! {
                    _ = tokio::time::sleep(BIND_RETRY) => {}
                    _ = shutdown.changed() => return,
                }
            }
        }
    };
    let bound = listener.local_addr().unwrap_or(addr);
    info!(addr = %bound, "mobile gateway listening");
    if crate::mobile::is_public_address(bound.ip()) {
        warn!(
            "mobile gateway listening on public address {bound}: {}",
            crate::mobile::PUBLIC_ADDRESS_NOTE
        );
    }
    gateway.add_bound(bound.to_string());
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    debug!(%peer, "mobile gateway connection");
                    tokio::spawn(connection::serve(gateway.clone(), stream, peer));
                }
                Err(err) => {
                    warn!(err = %err, "mobile gateway accept failed");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            },
            _ = shutdown.changed() => return,
        }
    }
}
