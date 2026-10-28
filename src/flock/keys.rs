//! Member keys: an Ed25519 key that signs log entries, and an
//! X25519 key for the Noise channel.

use std::io;
use std::path::Path;

use base64::Engine as _;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::mobile::store::{atomic_create, base64url, noise_public_key, random_bytes};

pub(crate) const MEMBER_KEY_FILE: &str = "member-key.json";
pub(crate) const MEMBER_ID_PREFIX: &str = "m_";

pub(crate) struct MemberKeys {
    pub sign: SigningKey,
    pub noise: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct MemberKeyFile {
    version: u32,
    sign: String,
    noise: String,
}

impl MemberKeys {
    pub(crate) fn generate() -> io::Result<Self> {
        Ok(Self {
            sign: SigningKey::from_bytes(&random_bytes::<32>()?),
            noise: random_bytes::<32>()?,
        })
    }

    pub(crate) fn sign_public(&self) -> [u8; 32] {
        self.sign.verifying_key().to_bytes()
    }

    pub(crate) fn noise_public(&self) -> io::Result<[u8; 32]> {
        noise_public_key(&self.noise)
    }

    pub(crate) fn member_id(&self) -> String {
        member_id(&self.sign_public())
    }
}

/// `m_` plus 16 base64url characters of SHA-256(signing public key).
pub(crate) fn member_id(sign_public: &[u8; 32]) -> String {
    format!(
        "{MEMBER_ID_PREFIX}{}",
        base64url(&Sha256::digest(sign_public)[..12])
    )
}

pub(crate) fn encode_key(key: &[u8; 32]) -> String {
    base64url(key)
}

pub(crate) fn decode_key(text: &str) -> Option<[u8; 32]> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .ok()?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

pub(crate) fn sign(key: &SigningKey, message: &[u8]) -> String {
    base64url(&key.sign(message).to_bytes())
}

/// Whether `signature` (base64url) is `public`'s signature over `message`.
pub(crate) fn verify(public: &str, message: &[u8], signature: &str) -> bool {
    let Some(public) = decode_key(public) else {
        return false;
    };
    let Ok(public) = VerifyingKey::from_bytes(&public) else {
        return false;
    };
    let Ok(signature) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(&signature) else {
        return false;
    };
    public.verify_strict(message, &signature).is_ok()
}

fn load(dir: &Path) -> io::Result<Option<MemberKeys>> {
    let text = match std::fs::read_to_string(dir.join(MEMBER_KEY_FILE)) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let file: MemberKeyFile = serde_json::from_str(&text)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "member key is malformed");
    let sign = decode_key(&file.sign).ok_or_else(invalid)?;
    let noise = decode_key(&file.noise).ok_or_else(invalid)?;
    Ok(Some(MemberKeys {
        sign: SigningKey::from_bytes(&sign),
        noise,
    }))
}

/// Loads this member's keys, creating them on first use.
pub(crate) fn load_or_create(dir: &Path) -> io::Result<MemberKeys> {
    if let Some(keys) = load(dir)? {
        return Ok(keys);
    }
    let keys = MemberKeys::generate()?;
    let file = MemberKeyFile {
        version: 1,
        sign: encode_key(&keys.sign.to_bytes()),
        noise: encode_key(&keys.noise),
    };
    let bytes = serde_json::to_vec(&file).map_err(io::Error::other)?;
    if atomic_create(&dir.join(MEMBER_KEY_FILE), &bytes)? {
        return Ok(keys);
    }
    load(dir)?.ok_or_else(|| io::Error::other("member key vanished while creating it"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_persist_and_signatures_verify() {
        let dir = crate::mobile::store::tests::temp_dir("flock-keys");
        let keys = load_or_create(&dir).unwrap();
        let again = load_or_create(&dir).unwrap();
        assert_eq!(keys.sign_public(), again.sign_public());
        assert_eq!(keys.noise, again.noise);
        assert!(keys.member_id().starts_with(MEMBER_ID_PREFIX));
        assert_eq!(keys.member_id().len(), MEMBER_ID_PREFIX.len() + 16);

        let public = encode_key(&keys.sign_public());
        let signature = sign(&keys.sign, b"hello");
        assert!(verify(&public, b"hello", &signature));
        assert!(!verify(&public, b"hellO", &signature));
        let other = MemberKeys::generate().unwrap();
        assert!(!verify(
            &encode_key(&other.sign_public()),
            b"hello",
            &signature
        ));
        assert!(!verify(&public, b"hello", "not base64!"));
    }
}
