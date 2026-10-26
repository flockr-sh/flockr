//! Noise transport for `mobile.v2` (`docs/mobile/protocol.md` §7). Pure: no
//! sockets and no shared state. Each connection task owns its [`Session`].
//!
//! `Noise_NK_25519_ChaChaPoly_SHA256`: the phone is the initiator and knows
//! the server's static public key from the pairing QR. Every WebSocket binary
//! frame after the handshake is one Noise transport message whose plaintext is
//! `flag || chunk`; a JSON message is the concatenation of its chunks.

use snow::params::NoiseParams;
use snow::{Builder, TransportState};

pub(crate) const PROTOCOL_NAME: &str = "Noise_NK_25519_ChaChaPoly_SHA256";
pub(crate) const PROLOGUE: &[u8] = b"flockr.mobile.v2";
/// Both handshake messages: 32-byte ephemeral key + 16-byte tag of an empty
/// payload.
pub(crate) const HANDSHAKE_MESSAGE_LEN: usize = 48;
/// Noise's maximum message size; also the WebSocket frame limit.
pub(crate) const MAX_FRAME_LEN: usize = 65535;
const TAG_LEN: usize = 16;
/// Largest chunk per frame: frame limit minus tag and flag byte.
pub(crate) const MAX_CHUNK_LEN: usize = MAX_FRAME_LEN - TAG_LEN - 1;
/// This chunk ends the JSON message.
pub(crate) const FLAG_LAST: u8 = 0x00;
/// More chunks of this JSON message follow.
pub(crate) const FLAG_MORE: u8 = 0x01;

/// Why an inbound transport frame was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenError {
    /// The frame did not decrypt (close 4000).
    Decrypt,
    /// Missing or unknown flag byte (close 4000).
    BadFlag,
    /// The message being reassembled passed the size cap (close 1009).
    TooBig,
}

fn params() -> Result<NoiseParams, snow::Error> {
    PROTOCOL_NAME.parse()
}

/// Responder builder for the server's static private key.
pub(crate) fn responder(private_key: &[u8; 32]) -> Result<Builder<'_>, snow::Error> {
    Builder::new(params()?)
        .local_private_key(private_key)?
        .prologue(PROLOGUE)
}

/// Reads handshake message 1 and returns message 2 plus the transport
/// session. `Err` (with a reason for logs) on any handshake failure.
pub(crate) fn accept(builder: Builder<'_>, msg1: &[u8]) -> Result<(Vec<u8>, Session), String> {
    if msg1.len() != HANDSHAKE_MESSAGE_LEN {
        return Err(format!(
            "handshake message is {} bytes, expected {HANDSHAKE_MESSAGE_LEN}",
            msg1.len()
        ));
    }
    let mut handshake = builder
        .build_responder()
        .map_err(|err| format!("could not start handshake: {err}"))?;
    let mut payload = [0_u8; HANDSHAKE_MESSAGE_LEN];
    handshake
        .read_message(msg1, &mut payload)
        .map_err(|err| format!("handshake message did not decrypt: {err}"))?;
    let mut msg2 = vec![0_u8; HANDSHAKE_MESSAGE_LEN];
    let len = handshake
        .write_message(&[], &mut msg2)
        .map_err(|err| format!("could not write handshake reply: {err}"))?;
    msg2.truncate(len);
    let transport = handshake
        .into_transport_mode()
        .map_err(|err| format!("could not finish handshake: {err}"))?;
    Ok((msg2, Session::new(transport)))
}

/// One side of an established channel: ciphers plus the message being
/// reassembled.
pub(crate) struct Session {
    transport: TransportState,
    partial: Vec<u8>,
    scratch: Vec<u8>,
}

impl Session {
    fn new(transport: TransportState) -> Self {
        Self {
            transport,
            partial: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// Encrypts one JSON message into one or more frames.
    pub(crate) fn seal(&mut self, message: &[u8]) -> Result<Vec<Vec<u8>>, snow::Error> {
        if message.is_empty() {
            return Ok(vec![self.seal_chunk(FLAG_LAST, &[])?]);
        }
        let mut frames = Vec::with_capacity(message.len().div_ceil(MAX_CHUNK_LEN));
        let mut chunks = message.chunks(MAX_CHUNK_LEN).peekable();
        while let Some(chunk) = chunks.next() {
            let flag = if chunks.peek().is_some() {
                FLAG_MORE
            } else {
                FLAG_LAST
            };
            frames.push(self.seal_chunk(flag, chunk)?);
        }
        Ok(frames)
    }

    fn seal_chunk(&mut self, flag: u8, chunk: &[u8]) -> Result<Vec<u8>, snow::Error> {
        self.scratch.clear();
        self.scratch.push(flag);
        self.scratch.extend_from_slice(chunk);
        let mut frame = vec![0_u8; self.scratch.len() + TAG_LEN];
        let len = self.transport.write_message(&self.scratch, &mut frame)?;
        frame.truncate(len);
        Ok(frame)
    }

    /// Decrypts one frame and adds its chunk to the message being
    /// reassembled. `Some` when the frame completed a message. The running
    /// total is checked against `cap` before each chunk is kept.
    pub(crate) fn open(&mut self, frame: &[u8], cap: usize) -> Result<Option<Vec<u8>>, OpenError> {
        self.scratch.clear();
        self.scratch.resize(frame.len(), 0);
        let len = self
            .transport
            .read_message(frame, &mut self.scratch)
            .map_err(|_| OpenError::Decrypt)?;
        let Some((&flag, chunk)) = self.scratch[..len].split_first() else {
            return Err(OpenError::BadFlag);
        };
        if flag != FLAG_LAST && flag != FLAG_MORE {
            return Err(OpenError::BadFlag);
        }
        if self.partial.len().saturating_add(chunk.len()) > cap {
            self.partial = Vec::new();
            return Err(OpenError::TooBig);
        }
        self.partial.extend_from_slice(chunk);
        if flag == FLAG_MORE {
            Ok(None)
        } else {
            Ok(Some(std::mem::take(&mut self.partial)))
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::{json, Value};
    use snow::HandshakeState;

    /// A phone-side initiator pinned to `server_public`.
    pub(crate) fn initiator(server_public: &[u8], ephemeral: Option<&[u8]>) -> HandshakeState {
        let mut builder = Builder::new(params().unwrap())
            .remote_public_key(server_public)
            .unwrap()
            .prologue(PROLOGUE)
            .unwrap();
        if let Some(ephemeral) = ephemeral {
            builder = builder.fixed_ephemeral_key_for_testing_only(ephemeral);
        }
        builder.build_initiator().unwrap()
    }

    pub(crate) fn write_msg1(handshake: &mut HandshakeState) -> Vec<u8> {
        let mut msg1 = vec![0_u8; 128];
        let len = handshake.write_message(&[], &mut msg1).unwrap();
        msg1.truncate(len);
        msg1
    }

    /// Reads message 2 and returns the phone's session.
    pub(crate) fn finish(mut handshake: HandshakeState, msg2: &[u8]) -> Session {
        let mut payload = [0_u8; 64];
        assert_eq!(handshake.read_message(msg2, &mut payload).unwrap(), 0);
        Session::new(handshake.into_transport_mode().unwrap())
    }

    fn key(start: u8) -> [u8; 32] {
        std::array::from_fn(|index| start + index as u8)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn connected() -> (Session, Session) {
        let server_key = key(0);
        let server_public = crate::mobile::store::noise_public_key(&server_key).unwrap();
        let mut client = initiator(&server_public, None);
        let msg1 = write_msg1(&mut client);
        let (msg2, server) = accept(responder(&server_key).unwrap(), &msg1).unwrap();
        (finish(client, &msg2), server)
    }

    #[test]
    fn handshake_messages_are_48_bytes_and_wrong_keys_fail() {
        let server_key = key(0);
        let server_public = crate::mobile::store::noise_public_key(&server_key).unwrap();
        let mut client = initiator(&server_public, None);
        let msg1 = write_msg1(&mut client);
        assert_eq!(msg1.len(), HANDSHAKE_MESSAGE_LEN);
        let (msg2, _) = accept(responder(&server_key).unwrap(), &msg1).unwrap();
        assert_eq!(msg2.len(), HANDSHAKE_MESSAGE_LEN);

        let other_public = crate::mobile::store::noise_public_key(&key(1)).unwrap();
        let msg1 = write_msg1(&mut initiator(&other_public, None));
        assert!(accept(responder(&server_key).unwrap(), &msg1).is_err());
        assert!(accept(responder(&server_key).unwrap(), &msg1[..47]).is_err());
        assert!(accept(responder(&server_key).unwrap(), &[0_u8; 48]).is_err());
    }

    #[test]
    fn messages_larger_than_one_chunk_round_trip_both_ways() {
        let (mut client, mut server) = connected();
        let big = "x".repeat(MAX_CHUNK_LEN * 2 + 10).into_bytes();
        let frames = server.seal(&big).unwrap();
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|frame| frame.len() <= MAX_FRAME_LEN));
        assert_eq!(frames[0].len(), MAX_FRAME_LEN);
        assert_eq!(client.open(&frames[0], usize::MAX), Ok(None));
        assert_eq!(client.open(&frames[1], usize::MAX), Ok(None));
        assert_eq!(client.open(&frames[2], usize::MAX), Ok(Some(big.clone())));

        let frames = client.seal(&big[..MAX_CHUNK_LEN + 1]).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(server.open(&frames[0], usize::MAX), Ok(None));
        assert_eq!(
            server.open(&frames[1], usize::MAX),
            Ok(Some(big[..MAX_CHUNK_LEN + 1].to_vec()))
        );

        // Exactly one chunk stays one frame.
        let frames = client.seal(&big[..MAX_CHUNK_LEN]).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), MAX_FRAME_LEN);
        assert!(server.open(&frames[0], usize::MAX).unwrap().is_some());
    }

    #[test]
    fn reassembly_stops_at_the_cap_and_bad_frames_fail() {
        let (mut client, mut server) = connected();
        let frames = client.seal(&vec![b'a'; MAX_CHUNK_LEN * 3]).unwrap();
        let cap = MAX_CHUNK_LEN * 2 - 1;
        assert_eq!(server.open(&frames[0], cap), Ok(None));
        assert_eq!(server.open(&frames[1], cap), Err(OpenError::TooBig));

        let mut frame = client.seal(b"{}").unwrap().remove(0);
        frame[3] ^= 1;
        assert_eq!(server.open(&frame, cap), Err(OpenError::Decrypt));

        let (mut client, mut server) = connected();
        let mut plain = vec![0x02];
        plain.extend_from_slice(b"{}");
        let mut frame = vec![0_u8; plain.len() + TAG_LEN];
        let len = client.transport.write_message(&plain, &mut frame).unwrap();
        assert_eq!(server.open(&frame[..len], cap), Err(OpenError::BadFlag));
        let len = client.transport.write_message(&[], &mut frame).unwrap();
        assert_eq!(server.open(&frame[..len], cap), Err(OpenError::BadFlag));
    }

    /// Builds the cross-implementation vector in
    /// `tests/fixtures/mobile-v2/noise-nk.json`.
    fn vector() -> Value {
        let responder_static = key(0x00);
        let initiator_ephemeral = key(0x20);
        let responder_ephemeral = key(0x40);
        let responder_public = crate::mobile::store::noise_public_key(&responder_static).unwrap();

        let mut client = initiator(&responder_public, Some(&initiator_ephemeral));
        let msg1 = write_msg1(&mut client);
        let builder = responder(&responder_static)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&responder_ephemeral);
        let (msg2, mut server) = accept(builder, &msg1).unwrap();
        let mut payload = [0_u8; 64];
        assert_eq!(client.read_message(&msg2, &mut payload).unwrap(), 0);
        let handshake_hash = client.get_handshake_hash().to_vec();
        let mut client = Session::new(client.into_transport_mode().unwrap());

        let messages = [
            (
                "initiator",
                r#"{"t":"hello","versions":[2],"app":{"version":"0.1.0","platform":"ios","build":"12"},"auth":{"device_id":"d_q8Zt3mVfR1xKp0aB","token":"fmd1_3mVfR1xKp0aBq8Zt3mVfR1xKp0aBq8Zt3mVfR1xKp0a"},"max_in_flight":2}"#,
            ),
            (
                "responder",
                r#"{"t":"error","code":"not_found","message":"pane p_9 not found","id":"r7"}"#,
            ),
            ("initiator", r#"{"t":"ping","ts":1761330000123}"#),
        ];
        let mut transport = Vec::new();
        for (from, message) in messages {
            let (sender, receiver) = if from == "initiator" {
                (&mut client, &mut server)
            } else {
                (&mut server, &mut client)
            };
            let frames = sender.seal(message.as_bytes()).unwrap();
            assert_eq!(frames.len(), 1);
            assert_eq!(
                receiver.open(&frames[0], usize::MAX),
                Ok(Some(message.as_bytes().to_vec()))
            );
            let mut plaintext = vec![FLAG_LAST];
            plaintext.extend_from_slice(message.as_bytes());
            transport.push(json!({
                "from": from,
                "plaintext_hex": hex(&plaintext),
                "ciphertext_hex": hex(&frames[0]),
            }));
        }
        json!({
            "protocol": PROTOCOL_NAME,
            "prologue_hex": hex(PROLOGUE),
            "responder_static_private_hex": hex(&responder_static),
            "responder_static_public_hex": hex(&responder_public),
            "initiator_ephemeral_private_hex": hex(&initiator_ephemeral),
            "responder_ephemeral_private_hex": hex(&responder_ephemeral),
            "msg1_hex": hex(&msg1),
            "msg2_hex": hex(&msg2),
            "handshake_hash_hex": hex(&handshake_hash),
            "transport": transport,
        })
    }

    /// Pretty JSON with keys in the documented order (serde_json sorts them).
    fn render(vector: &Value) -> String {
        const KEYS: [&str; 9] = [
            "protocol",
            "prologue_hex",
            "responder_static_private_hex",
            "responder_static_public_hex",
            "initiator_ephemeral_private_hex",
            "responder_ephemeral_private_hex",
            "msg1_hex",
            "msg2_hex",
            "handshake_hash_hex",
        ];
        let mut out = String::from("{\n");
        for key in KEYS {
            out.push_str(&format!("  \"{key}\": {},\n", vector[key]));
        }
        out.push_str("  \"transport\": [\n");
        let entries = vector["transport"].as_array().unwrap();
        for (index, entry) in entries.iter().enumerate() {
            out.push_str(&format!(
                "    {{ \"from\": {}, \"plaintext_hex\": {}, \"ciphertext_hex\": {} }}{}\n",
                entry["from"],
                entry["plaintext_hex"],
                entry["ciphertext_hex"],
                if index + 1 < entries.len() { "," } else { "" }
            ));
        }
        out.push_str("  ]\n}\n");
        out
    }

    /// Set `FLOCKR_BLESS_NOISE_VECTOR=1` to rewrite the fixture.
    #[test]
    fn noise_nk_fixture_matches_snow() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mobile-v2/noise-nk.json");
        let expected = vector();
        if std::env::var_os("FLOCKR_BLESS_NOISE_VECTOR").is_some() {
            std::fs::write(&path, render(&expected)).unwrap();
        }
        let fixture: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            fixture,
            expected,
            "{} is stale; rerun with FLOCKR_BLESS_NOISE_VECTOR=1",
            path.display()
        );
    }
}
