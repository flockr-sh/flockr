//! This machine's X25519 key for the Noise channel. The hub knows members by
//! their public key; members know the hub by its.

use std::io;
use std::path::Path;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::mobile::store::{atomic_create, base64url, noise_public_key, random_bytes};

pub(crate) const MEMBER_KEY_FILE: &str = "member-key.json";

pub(crate) struct MemberKeys {
    pub noise: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct MemberKeyFile {
    version: u32,
    noise: String,
}

impl MemberKeys {
    pub(crate) fn generate() -> io::Result<Self> {
        Ok(Self {
            noise: random_bytes::<32>()?,
        })
    }

    pub(crate) fn noise_public(&self) -> io::Result<[u8; 32]> {
        noise_public_key(&self.noise)
    }
}

/// A random ID: `prefix` plus 16 base64url characters.
pub(crate) fn new_id(prefix: &str) -> io::Result<String> {
    Ok(format!("{prefix}{}", base64url(&random_bytes::<12>()?)))
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

fn load(dir: &Path) -> io::Result<Option<MemberKeys>> {
    let text = match std::fs::read_to_string(dir.join(MEMBER_KEY_FILE)) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let file: MemberKeyFile = serde_json::from_str(&text)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let noise = decode_key(&file.noise)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "member key is malformed"))?;
    Ok(Some(MemberKeys { noise }))
}

/// Loads this machine's key, creating it on first use.
pub(crate) fn load_or_create(dir: &Path) -> io::Result<MemberKeys> {
    if let Some(keys) = load(dir)? {
        return Ok(keys);
    }
    let keys = MemberKeys::generate()?;
    let file = MemberKeyFile {
        version: 1,
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
    fn keys_persist_and_ids_are_random() {
        let dir = crate::mobile::store::tests::temp_dir("flock-keys");
        let keys = load_or_create(&dir).unwrap();
        let again = load_or_create(&dir).unwrap();
        assert_eq!(keys.noise, again.noise);
        let encoded = encode_key(&keys.noise_public().unwrap());
        assert_eq!(decode_key(&encoded), Some(keys.noise_public().unwrap()));
        assert_eq!(decode_key("not base64!"), None);
        let id = new_id("m_").unwrap();
        assert_eq!(id.len(), 18);
        assert_ne!(id, new_id("m_").unwrap());
    }
}
