//! The owner key: the group's trust root. Stored only on the owner device,
//! encrypted with a passphrase (Argon2id, then ChaCha20-Poly1305).

use std::io;
use std::path::Path;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead as _, KeyInit as _, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::keys::{decode_key, encode_key};
use crate::mobile::store::{atomic_create, base64url, random_bytes};

pub(crate) const OWNER_KEY_FILE: &str = "owner-key.json";
const AAD: &[u8] = b"flockr.flock.owner-key.v1";
pub(crate) const MIN_PASSPHRASE_CHARS: usize = 8;

#[cfg(not(test))]
const KDF_MEMORY_KIB: u32 = 64 * 1024;
#[cfg(test)]
const KDF_MEMORY_KIB: u32 = 64;
#[cfg(not(test))]
const KDF_ITERATIONS: u32 = 3;
#[cfg(test)]
const KDF_ITERATIONS: u32 = 1;

#[derive(Serialize, Deserialize)]
struct OwnerKeyFile {
    version: u32,
    public: String,
    salt: String,
    nonce: String,
    ciphertext: String,
    memory_kib: u32,
    iterations: u32,
}

#[derive(Debug)]
pub(crate) enum UnlockError {
    Missing,
    WrongPassphrase,
    Io(io::Error),
}

impl std::fmt::Display for UnlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("this machine does not hold the owner key"),
            Self::WrongPassphrase => f.write_str("wrong passphrase"),
            Self::Io(err) => write!(f, "could not read the owner key: {err}"),
        }
    }
}

fn derive(
    passphrase: &str,
    salt: &[u8],
    memory_kib: u32,
    iterations: u32,
) -> io::Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(memory_kib, iterations, 1, Some(32))
        .map_err(|err| io::Error::other(err.to_string()))?;
    let mut key = Zeroizing::new([0_u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
        .map_err(|err| io::Error::other(err.to_string()))?;
    Ok(key)
}

fn decode(text: &str) -> io::Result<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Creates the owner key. Fails if one already exists.
pub(crate) fn create(dir: &Path, passphrase: &str) -> io::Result<SigningKey> {
    let key = SigningKey::from_bytes(&random_bytes::<32>()?);
    let salt = random_bytes::<16>()?;
    let nonce = random_bytes::<12>()?;
    let wrap = derive(passphrase, &salt, KDF_MEMORY_KIB, KDF_ITERATIONS)?;
    let secret = Zeroizing::new(key.to_bytes());
    let ciphertext = ChaCha20Poly1305::new(Key::from_slice(wrap.as_ref()))
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: secret.as_ref(),
                aad: AAD,
            },
        )
        .map_err(|_| io::Error::other("could not encrypt the owner key"))?;
    let file = OwnerKeyFile {
        version: 1,
        public: encode_key(&key.verifying_key().to_bytes()),
        salt: base64url(&salt),
        nonce: base64url(&nonce),
        ciphertext: base64url(&ciphertext),
        memory_kib: KDF_MEMORY_KIB,
        iterations: KDF_ITERATIONS,
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    if !atomic_create(&dir.join(OWNER_KEY_FILE), &bytes)? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "an owner key already exists on this machine",
        ));
    }
    Ok(key)
}

fn load(dir: &Path) -> Result<OwnerKeyFile, UnlockError> {
    let text = match std::fs::read_to_string(dir.join(OWNER_KEY_FILE)) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Err(UnlockError::Missing),
        Err(err) => return Err(UnlockError::Io(err)),
    };
    serde_json::from_str(&text)
        .map_err(|err| UnlockError::Io(io::Error::new(io::ErrorKind::InvalidData, err)))
}

/// The owner public key held on this machine, if any.
pub(crate) fn public(dir: &Path) -> Option<String> {
    load(dir).ok().map(|file| file.public)
}

pub(crate) fn unlock(dir: &Path, passphrase: &str) -> Result<SigningKey, UnlockError> {
    let file = load(dir)?;
    let salt = decode(&file.salt).map_err(UnlockError::Io)?;
    let nonce = decode(&file.nonce).map_err(UnlockError::Io)?;
    let ciphertext = decode(&file.ciphertext).map_err(UnlockError::Io)?;
    if nonce.len() != 12 {
        return Err(UnlockError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "owner key nonce is malformed",
        )));
    }
    let wrap =
        derive(passphrase, &salt, file.memory_kib, file.iterations).map_err(UnlockError::Io)?;
    let secret = Zeroizing::new(
        ChaCha20Poly1305::new(Key::from_slice(wrap.as_ref()))
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: AAD,
                },
            )
            .map_err(|_| UnlockError::WrongPassphrase)?,
    );
    let secret = <[u8; 32]>::try_from(secret.as_slice()).map_err(|_| {
        UnlockError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "owner key is malformed",
        ))
    })?;
    let key = SigningKey::from_bytes(&secret);
    if Some(key.verifying_key().to_bytes()) != decode_key(&file.public) {
        return Err(UnlockError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "owner key does not match its public key",
        )));
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_key_unlocks_only_with_its_passphrase() {
        let dir = crate::mobile::store::tests::temp_dir("flock-owner");
        assert!(matches!(
            unlock(&dir, "whatever"),
            Err(UnlockError::Missing)
        ));
        let key = create(&dir, "correct horse").unwrap();
        assert!(create(&dir, "again").is_err());
        assert_eq!(
            public(&dir),
            Some(encode_key(&key.verifying_key().to_bytes()))
        );
        let unlocked = unlock(&dir, "correct horse").unwrap();
        assert_eq!(unlocked.to_bytes(), key.to_bytes());
        assert!(matches!(
            unlock(&dir, "wrong horse"),
            Err(UnlockError::WrongPassphrase)
        ));
        let text = std::fs::read_to_string(dir.join(OWNER_KEY_FILE)).unwrap();
        assert!(!text.contains(&base64url(&key.to_bytes())));
    }
}
