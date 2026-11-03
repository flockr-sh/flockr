//! The encrypted channel from a flock member to the hub: WebSocket at
//! `/flock/v1` carrying `Noise_IK_25519_ChaChaPoly_SHA256`. The member knows
//! the hub's static key from the invite; the hub learns the member's in the
//! handshake and checks it against its member list. The
//! prologue binds the group ID, so a handshake for another group fails.
//!
//! After the handshake, frames and chunking match `mobile.v2`
//! (`crate::server::mobile::noise::Session`); each message is JSON.

use std::io;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use snow::{Builder, HandshakeState};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::server::mobile::noise::{OpenError, Session, MAX_FRAME_LEN};

pub(crate) const PATH: &str = "/flock/v1";
pub(crate) const SUBPROTOCOL: &str = "flockr.flock.v1";
pub(crate) const PROTOCOL_NAME: &str = "Noise_IK_25519_ChaChaPoly_SHA256";
/// e, encrypted s, and the tag of an empty payload.
const MSG1_LEN: usize = 32 + 48 + 16;
/// e and the tag of an empty payload.
const MSG2_LEN: usize = 32 + 16;
/// Cap on one reassembled message.
pub(crate) const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

fn prologue(group: &str) -> Vec<u8> {
    format!("{SUBPROTOCOL}:{group}").into_bytes()
}

fn builder<'a>(
    local_private: &'a [u8; 32],
    prologue: &'a [u8],
) -> Result<Builder<'a>, snow::Error> {
    Builder::new(PROTOCOL_NAME.parse()?)
        .local_private_key(local_private)?
        .prologue(prologue)
}

/// Short code both screens show during a join, from the handshake hash:
/// eight digits as `1234-5678`.
pub(crate) fn sas(handshake_hash: &[u8]) -> String {
    let mut bytes = [0_u8; 4];
    bytes.copy_from_slice(&handshake_hash[..4]);
    let number = u32::from_be_bytes(bytes) % 100_000_000;
    format!("{:04}-{:04}", number / 10_000, number % 10_000)
}

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_frame_size(Some(MAX_FRAME_LEN + 64))
        .max_message_size(Some(MAX_FRAME_LEN + 64))
}

fn other(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

/// One established channel. `recv` is cancel-safe: a partly reassembled
/// message stays in the session.
pub(crate) struct Channel {
    ws: WebSocketStream<TcpStream>,
    session: Session,
    pub remote_static: [u8; 32],
    pub handshake_hash: Vec<u8>,
}

#[allow(clippy::result_large_err)] // The error type is fixed by tungstenite's callback trait.
fn check_upgrade(request: &Request, mut response: Response) -> Result<Response, ErrorResponse> {
    let reject = |status: StatusCode, body: &str| {
        let mut error = ErrorResponse::new(Some(body.to_owned()));
        *error.status_mut() = status;
        error
    };
    if request.uri().path() != PATH {
        return Err(reject(StatusCode::NOT_FOUND, "not found"));
    }
    let offered = request
        .headers()
        .get_all("Sec-WebSocket-Protocol")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|protocol| protocol.trim() == SUBPROTOCOL);
    if !offered {
        return Err(reject(StatusCode::BAD_REQUEST, "missing subprotocol"));
    }
    response.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(SUBPROTOCOL),
    );
    Ok(response)
}

async fn next_binary(ws: &mut WebSocketStream<TcpStream>) -> io::Result<Option<Vec<u8>>> {
    loop {
        match ws.next().await {
            None | Some(Ok(Message::Close(_))) => return Ok(None),
            Some(Ok(Message::Binary(bytes))) => return Ok(Some(bytes.to_vec())),
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
            Some(Ok(Message::Text(_))) => return Err(other("text frame on the flock channel")),
            Some(Err(err)) => return Err(other(err.to_string())),
        }
    }
}

impl Channel {
    /// Connects to `addr` (`host:port`) as the initiator.
    pub(crate) async fn connect(
        addr: &str,
        local_private: &[u8; 32],
        remote_public: &[u8; 32],
        group: &str,
    ) -> io::Result<Self> {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            let stream = TcpStream::connect(addr).await?;
            let _ = stream.set_nodelay(true);
            let mut request = format!("ws://{addr}{PATH}")
                .into_client_request()
                .map_err(|err| other(err.to_string()))?;
            request.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                HeaderValue::from_static(SUBPROTOCOL),
            );
            let (mut ws, _) =
                tokio_tungstenite::client_async_with_config(request, stream, Some(ws_config()))
                    .await
                    .map_err(|err| other(err.to_string()))?;
            let prologue = prologue(group);
            let mut handshake = builder(local_private, &prologue)
                .and_then(|builder| builder.remote_public_key(remote_public))
                .and_then(Builder::build_initiator)
                .map_err(|err| other(err.to_string()))?;
            let mut msg1 = vec![0_u8; MSG1_LEN];
            let len = handshake
                .write_message(&[], &mut msg1)
                .map_err(|err| other(err.to_string()))?;
            msg1.truncate(len);
            ws.send(Message::Binary(msg1.into()))
                .await
                .map_err(|err| other(err.to_string()))?;
            let msg2 = next_binary(&mut ws)
                .await?
                .ok_or_else(|| other("peer closed during the handshake"))?;
            let mut payload = [0_u8; MSG2_LEN];
            handshake
                .read_message(&msg2, &mut payload)
                .map_err(|_| other("handshake failed: wrong key or wrong group"))?;
            Self::finish(ws, handshake)
        })
        .await
        .map_err(|_| other(format!("timed out connecting to {addr}")))?
    }

    /// Accepts a connection as the responder.
    pub(crate) async fn accept(
        stream: TcpStream,
        local_private: &[u8; 32],
        group: &str,
    ) -> io::Result<Self> {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            let mut ws = tokio_tungstenite::accept_hdr_async_with_config(
                stream,
                check_upgrade,
                Some(ws_config()),
            )
            .await
            .map_err(|err| other(err.to_string()))?;
            let msg1 = next_binary(&mut ws)
                .await?
                .ok_or_else(|| other("peer closed during the handshake"))?;
            if msg1.len() != MSG1_LEN {
                return Err(other("handshake message has the wrong length"));
            }
            let prologue = prologue(group);
            let mut handshake = builder(local_private, &prologue)
                .and_then(Builder::build_responder)
                .map_err(|err| other(err.to_string()))?;
            let mut payload = [0_u8; MSG1_LEN];
            handshake
                .read_message(&msg1, &mut payload)
                .map_err(|_| other("handshake failed: wrong key or wrong group"))?;
            let mut msg2 = vec![0_u8; MSG2_LEN];
            let len = handshake
                .write_message(&[], &mut msg2)
                .map_err(|err| other(err.to_string()))?;
            msg2.truncate(len);
            ws.send(Message::Binary(msg2.into()))
                .await
                .map_err(|err| other(err.to_string()))?;
            Self::finish(ws, handshake)
        })
        .await
        .map_err(|_| other("handshake timed out"))?
    }

    fn finish(ws: WebSocketStream<TcpStream>, handshake: HandshakeState) -> io::Result<Self> {
        let remote_static = handshake
            .get_remote_static()
            .and_then(|key| <[u8; 32]>::try_from(key).ok())
            .ok_or_else(|| other("peer sent no static key"))?;
        let handshake_hash = handshake.get_handshake_hash().to_vec();
        let transport = handshake
            .into_transport_mode()
            .map_err(|err| other(err.to_string()))?;
        Ok(Self {
            ws,
            session: Session::new(transport),
            remote_static,
            handshake_hash,
        })
    }

    pub(crate) async fn send(&mut self, message: &Value) -> io::Result<()> {
        let text = serde_json::to_vec(message).map_err(io::Error::other)?;
        let frames = self
            .session
            .seal(&text)
            .map_err(|err| other(err.to_string()))?;
        let ws = &mut self.ws;
        let write = async move {
            for frame in frames {
                ws.feed(Message::Binary(frame.into())).await?;
            }
            ws.flush().await
        };
        tokio::time::timeout(WRITE_TIMEOUT, write)
            .await
            .map_err(|_| other("write timed out"))?
            .map_err(|err| other(err.to_string()))
    }

    /// The next JSON message, or `None` when the peer closed.
    pub(crate) async fn recv(&mut self) -> io::Result<Option<Value>> {
        loop {
            let Some(frame) = next_binary(&mut self.ws).await? else {
                return Ok(None);
            };
            match self.session.open(&frame, MAX_MESSAGE_BYTES) {
                Ok(Some(bytes)) => {
                    return serde_json::from_slice(&bytes)
                        .map(Some)
                        .map_err(|err| other(format!("bad message: {err}")));
                }
                Ok(None) => {}
                Err(OpenError::TooBig) => return Err(other("message too large")),
                Err(OpenError::Decrypt | OpenError::BadFlag) => {
                    return Err(other("frame did not decrypt"))
                }
            }
        }
    }

    pub(crate) async fn close(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.ws.close(None)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(start: u8) -> [u8; 32] {
        std::array::from_fn(|index| start.wrapping_add(index as u8))
    }

    #[tokio::test]
    async fn ik_channel_authenticates_both_ends_and_binds_the_group() {
        let server_key = key(1);
        let client_key = key(50);
        let server_public = crate::mobile::store::noise_public_key(&server_key).unwrap();
        let client_public = crate::mobile::store::noise_public_key(&client_key).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut channel = Channel::accept(stream, &server_key, "g1").await.unwrap();
            let got = channel.recv().await.unwrap().unwrap();
            channel.send(&json!({"echo": got})).await.unwrap();
            (channel.remote_static, channel.handshake_hash.clone())
        });
        let mut client = Channel::connect(&addr, &client_key, &server_public, "g1")
            .await
            .unwrap();
        let big = "x".repeat(200_000);
        client
            .send(&json!({"t": "hello", "big": big}))
            .await
            .unwrap();
        let reply = client.recv().await.unwrap().unwrap();
        assert_eq!(reply["echo"]["big"].as_str().unwrap().len(), 200_000);
        let (seen_client, server_hash) = server.await.unwrap();
        assert_eq!(seen_client, client_public);
        assert_eq!(server_hash, client.handshake_hash);
        assert_eq!(sas(&server_hash), sas(&client.handshake_hash));
        assert_eq!(sas(&server_hash).len(), 9);

        // Wrong group: the handshake fails.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            Channel::accept(stream, &server_key, "g1").await.is_err()
        });
        assert!(Channel::connect(&addr, &client_key, &server_public, "g2")
            .await
            .is_err());
        assert!(server.await.unwrap());
    }
}
