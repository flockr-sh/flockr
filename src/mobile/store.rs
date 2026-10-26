//! Mobile gateway state files: server identity, Noise static key, pending
//! pairing code, paired devices, and the gateway's bound addresses.
//!
//! All files live in `state_dir()/mobile/`, are written atomically with mode
//! 0600, and are shared by the CLI and the server.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

pub(crate) const SERVER_ID_FILE: &str = "server-id";
pub(crate) const NOISE_KEY_FILE: &str = "noise-key";
pub(crate) const PAIRING_FILE: &str = "pairing.json";
pub(crate) const DEVICES_FILE: &str = "devices.json";
pub(crate) const GATEWAY_FILE: &str = "gateway.json";

pub(crate) const DEVICE_TOKEN_PREFIX: &str = "fmd1_";
pub(crate) const DEVICE_ID_PREFIX: &str = "d_";
/// Failed code attempts that kill a pending pairing code.
pub(crate) const MAX_PAIRING_FAILURES: u32 = 5;
pub(crate) const MAX_DEVICE_NAME_CHARS: usize = 64;

/// Directory holding all mobile state for this flockr install.
pub(crate) fn mobile_dir() -> PathBuf {
    crate::config::state_dir().join("mobile")
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn ensure_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    crate::ipc::restrict_socket_permissions(dir, 0o700)
}

/// Writes `bytes` to a fresh temp file (mode 0600, fsynced) next to `path`
/// and returns the temp path.
fn write_temp(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    ensure_dir(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mobile");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let tmp_path = parent.join(format!(".{file_name}.{}.{nanos}.tmp", std::process::id()));
    let written = (|| {
        let mut file = fs::File::create(&tmp_path)?;
        crate::ipc::restrict_socket_permissions(&tmp_path, 0o600)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(err) = written {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(tmp_path)
}

/// Writes `bytes` to `path` through a temp file in the same directory:
/// mode 0600, fsync, rename.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp_path = write_temp(path, bytes)?;
    fs::rename(&tmp_path, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp_path);
    })
}

/// Like [`atomic_write`], but never replaces an existing file: `Ok(false)`
/// when `path` already exists.
fn atomic_create(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    let tmp_path = write_temp(path, bytes)?;
    let linked = fs::hard_link(&tmp_path, path);
    let _ = fs::remove_file(&tmp_path);
    match linked {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err),
    }
}

pub(crate) fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|err| io::Error::other(err.to_string()))?;
    Ok(bytes)
}

pub(crate) fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn decode_hex32(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0_u8; 32];
    for (index, chunk) in bytes.chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        out[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(out)
}

/// Compares equal-length byte strings without an early exit.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Whether `presented` hashes to the stored lowercase-hex SHA-256 digest.
pub(crate) fn secret_matches(presented: &str, stored_sha256_hex: &str) -> bool {
    let Some(stored) = decode_hex32(stored_sha256_hex) else {
        return false;
    };
    let presented = Sha256::digest(presented.as_bytes());
    constant_time_eq(presented.as_slice(), &stored)
}

/// A fresh 128-bit bootstrap code (22 characters).
pub(crate) fn generate_pairing_code() -> io::Result<String> {
    Ok(base64url(&random_bytes::<16>()?))
}

/// A fresh 256-bit device token: `fmd1_` + 43 characters.
pub(crate) fn generate_device_token() -> io::Result<String> {
    Ok(format!(
        "{DEVICE_TOKEN_PREFIX}{}",
        base64url(&random_bytes::<32>()?)
    ))
}

/// A fresh device id: `d_` + 16 characters.
pub(crate) fn generate_device_id() -> io::Result<String> {
    Ok(format!(
        "{DEVICE_ID_PREFIX}{}",
        base64url(&random_bytes::<12>()?)
    ))
}

/// Loads the server identity secret, creating it on first use.
pub(crate) fn load_or_create_server_secret(dir: &Path) -> io::Result<Vec<u8>> {
    let path = dir.join(SERVER_ID_FILE);
    match fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => return Ok(content.trim().as_bytes().to_vec()),
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let secret = random_bytes::<32>()?;
    let mut hex = String::with_capacity(64);
    for byte in secret {
        hex.push_str(&format!("{byte:02x}"));
    }
    atomic_write(&path, hex.as_bytes())?;
    Ok(hex.into_bytes())
}

/// Public fingerprint of the server identity: 22 base64url characters.
pub(crate) fn server_fingerprint(secret: &[u8]) -> String {
    let digest = Sha256::digest(secret);
    base64url(&digest[..16])
}

pub(crate) fn load_or_create_server_fingerprint(dir: &Path) -> io::Result<String> {
    Ok(server_fingerprint(&load_or_create_server_secret(dir)?))
}

/// Reads the gateway's Noise static private key. `None` when it does not
/// exist yet.
pub(crate) fn load_noise_key(dir: &Path) -> io::Result<Option<[u8; 32]>> {
    match fs::read(dir.join(NOISE_KEY_FILE)) {
        Ok(bytes) => <[u8; 32]>::try_from(bytes.as_slice())
            .map(Some)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{} is {} bytes, expected 32; remove it and pair phones again",
                        dir.join(NOISE_KEY_FILE).display(),
                        bytes.len()
                    ),
                )
            }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Loads the Noise static private key (32 raw bytes), creating it on first
/// use. The CLI and the server may race here; the first writer wins and both
/// end up with the same key.
pub(crate) fn load_or_create_noise_key(dir: &Path) -> io::Result<[u8; 32]> {
    if let Some(key) = load_noise_key(dir)? {
        return Ok(key);
    }
    let key = random_bytes::<32>()?;
    if atomic_create(&dir.join(NOISE_KEY_FILE), &key)? {
        return Ok(key);
    }
    load_noise_key(dir)?.ok_or_else(|| io::Error::other("noise key vanished while creating it"))
}

/// X25519 public key for a Noise static private key.
pub(crate) fn noise_public_key(private: &[u8; 32]) -> io::Result<[u8; 32]> {
    use snow::resolvers::{CryptoResolver, DefaultResolver};
    let mut dh = DefaultResolver
        .resolve_dh(&snow::params::DHChoice::Curve25519)
        .ok_or_else(|| io::Error::other("X25519 is not available"))?;
    dh.set(private);
    <[u8; 32]>::try_from(dh.pubkey())
        .map_err(|_| io::Error::other("X25519 public key is not 32 bytes"))
}

/// Key fingerprint shown by `flockr mobile status`: base64url of the first
/// 16 bytes of SHA-256(public key).
pub(crate) fn noise_key_fingerprint(public: &[u8; 32]) -> String {
    base64url(&Sha256::digest(public)[..16])
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PairingRecord {
    pub version: u32,
    pub code_sha256: String,
    pub created_at: u64,
    pub expires_at: u64,
    #[serde(default)]
    pub failed_attempts: u32,
    #[serde(default)]
    pub consumed_by: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl PairingRecord {
    pub(crate) fn new(code: &str, now: u64, ttl_secs: u64) -> Self {
        Self {
            version: 1,
            code_sha256: sha256_hex(code),
            created_at: now,
            expires_at: now.saturating_add(ttl_secs),
            failed_attempts: 0,
            consumed_by: None,
            extra: serde_json::Map::new(),
        }
    }

    /// Whether this record can still be redeemed.
    pub(crate) fn is_live(&self, now: u64) -> bool {
        self.consumed_by.is_none()
            && now < self.expires_at
            && self.failed_attempts < MAX_PAIRING_FAILURES
    }
}

pub(crate) fn load_pairing(dir: &Path) -> io::Result<Option<PairingRecord>> {
    match fs::read(dir.join(PAIRING_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

pub(crate) fn save_pairing(dir: &Path, record: &PairingRecord) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
    atomic_write(&dir.join(PAIRING_FILE), &json)
}

pub(crate) fn remove_pairing(dir: &Path) -> io::Result<()> {
    match fs::remove_file(dir.join(PAIRING_FILE)) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PushRecord {
    pub expo_token: String,
    pub registered_at: u64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeviceRecord {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub token_sha256: String,
    pub created_at: u64,
    #[serde(default)]
    pub last_seen_at: u64,
    #[serde(default)]
    pub push: Option<PushRecord>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DevicesFile {
    pub version: u32,
    #[serde(default)]
    pub devices: Vec<DeviceRecord>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for DevicesFile {
    fn default() -> Self {
        Self {
            version: 1,
            devices: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }
}

impl DevicesFile {
    pub(crate) fn find(&self, device_id: &str) -> Option<&DeviceRecord> {
        self.devices
            .iter()
            .find(|device| device.device_id == device_id)
    }

    pub(crate) fn find_mut(&mut self, device_id: &str) -> Option<&mut DeviceRecord> {
        self.devices
            .iter_mut()
            .find(|device| device.device_id == device_id)
    }

    /// The device whose id and token both match, compared in constant time.
    pub(crate) fn authenticate(&self, device_id: &str, token: &str) -> Option<&DeviceRecord> {
        // Hash even for unknown ids so timing does not reveal which part failed.
        let device = self.find(device_id);
        let stored = device.map_or(
            "0000000000000000000000000000000000000000000000000000000000000000",
            |device| device.token_sha256.as_str(),
        );
        let matches = secret_matches(token, stored);
        device.filter(|_| matches)
    }

    pub(crate) fn remove(&mut self, device_id: &str) -> Option<DeviceRecord> {
        let index = self
            .devices
            .iter()
            .position(|device| device.device_id == device_id)?;
        Some(self.devices.remove(index))
    }
}

/// Loads paired devices. Missing means none. An unparsable file is renamed
/// to `devices.json.bad-<unix>` so a later write does not destroy it.
pub(crate) fn load_devices(dir: &Path) -> DevicesFile {
    let path = dir.join(DEVICES_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return DevicesFile::default(),
        Err(err) => {
            warn!(path = %path.display(), err = %err, "could not read mobile devices file");
            return DevicesFile::default();
        }
    };
    match serde_json::from_slice::<DevicesFile>(&bytes) {
        Ok(file) => file,
        Err(err) => {
            let backup = dir.join(format!("{DEVICES_FILE}.bad-{}", now_unix()));
            warn!(
                path = %path.display(),
                backup = %backup.display(),
                err = %err,
                "mobile devices file is unparsable; moving it aside"
            );
            if let Err(err) = fs::rename(&path, &backup) {
                warn!(err = %err, "could not move unparsable mobile devices file aside");
            }
            DevicesFile::default()
        }
    }
}

pub(crate) fn save_devices(dir: &Path, devices: &DevicesFile) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(devices).map_err(io::Error::other)?;
    atomic_write(&dir.join(DEVICES_FILE), &json)
}

/// Re-reads `devices.json`, applies `change`, and writes it back.
pub(crate) fn update_devices<T>(
    dir: &Path,
    change: impl FnOnce(&mut DevicesFile) -> T,
) -> io::Result<(T, DevicesFile)> {
    let mut devices = load_devices(dir);
    let result = change(&mut devices);
    save_devices(dir, &devices)?;
    Ok((result, devices))
}

pub(crate) fn devices_modified(dir: &Path) -> Option<SystemTime> {
    fs::metadata(dir.join(DEVICES_FILE))
        .and_then(|metadata| metadata.modified())
        .ok()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GatewayRecord {
    pub version: u32,
    pub pid: u32,
    pub bound: Vec<String>,
    pub started_at: u64,
}

pub(crate) fn load_gateway(dir: &Path) -> Option<GatewayRecord> {
    let bytes = fs::read(dir.join(GATEWAY_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn save_gateway(dir: &Path, record: &GatewayRecord) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
    atomic_write(&dir.join(GATEWAY_FILE), &json)
}

/// Removes `gateway.json` if it still belongs to this process.
pub(crate) fn remove_gateway_if_owned(dir: &Path, pid: u32) {
    if load_gateway(dir).is_some_and(|record| record.pid == pid) {
        let _ = fs::remove_file(dir.join(GATEWAY_FILE));
    }
}

/// Trims a device name to 1..=64 characters; empty becomes "Phone".
pub(crate) fn clean_device_name(name: &str) -> String {
    let name: String = name
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_DEVICE_NAME_CHARS)
        .collect();
    if name.is_empty() {
        "Phone".to_owned()
    } else {
        name
    }
}

/// The last six characters of a secret, for logs.
pub(crate) fn redact(secret: &str) -> String {
    let tail: String = secret
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flockr-mobile-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn tokens_ids_and_codes_have_the_documented_shape() {
        let code = generate_pairing_code().unwrap();
        assert_eq!(code.len(), 22);
        let token = generate_device_token().unwrap();
        assert!(token.starts_with("fmd1_"));
        assert_eq!(token.len(), 5 + 43);
        let id = generate_device_id().unwrap();
        assert!(id.starts_with("d_"));
        assert_eq!(id.len(), 2 + 16);
        assert_ne!(generate_device_token().unwrap(), token);
    }

    #[test]
    fn secret_hash_matches_only_the_same_secret() {
        let hash = sha256_hex("fmd1_abc");
        assert_eq!(hash.len(), 64);
        assert!(secret_matches("fmd1_abc", &hash));
        assert!(!secret_matches("fmd1_abd", &hash));
        assert!(!secret_matches("fmd1_abc", "not-hex"));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn server_fingerprint_is_stable_and_22_chars() {
        let dir = temp_dir("sid");
        let first = load_or_create_server_fingerprint(&dir).unwrap();
        let second = load_or_create_server_fingerprint(&dir).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 22);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(SERVER_ID_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn noise_key_is_created_once_with_mode_0600() {
        let dir = temp_dir("noise-key");
        assert_eq!(load_noise_key(&dir).unwrap(), None);
        let first = load_or_create_noise_key(&dir).unwrap();
        let second = load_or_create_noise_key(&dir).unwrap();
        assert_eq!(first, second);
        assert_eq!(fs::read(dir.join(NOISE_KEY_FILE)).unwrap(), first);
        // Only the key file is left; the temp file is gone.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(NOISE_KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::write(dir.join(NOISE_KEY_FILE), b"short").unwrap();
        assert!(load_or_create_noise_key(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn noise_public_key_and_fingerprint() {
        // RFC 7748 section 6.1, Alice's key pair.
        let private: [u8; 32] = [
            0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2,
            0x66, 0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5,
            0x1d, 0xb9, 0x2c, 0x2a,
        ];
        let public = noise_public_key(&private).unwrap();
        let hex: String = public.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"
        );
        let fingerprint = noise_key_fingerprint(&public);
        assert_eq!(fingerprint.len(), 22);
        assert_eq!(fingerprint, base64url(&Sha256::digest(public)[..16]));
    }

    #[test]
    fn devices_round_trip_keeps_unknown_fields_and_authenticates() {
        let dir = temp_dir("devices");
        let token = generate_device_token().unwrap();
        fs::write(
            dir.join(DEVICES_FILE),
            format!(
                r#"{{"version":1,"future":true,"devices":[{{"device_id":"d_1","name":"Phone","platform":"ios","token_sha256":"{}","created_at":1,"extra_field":"kept"}}]}}"#,
                sha256_hex(&token)
            ),
        )
        .unwrap();
        let devices = load_devices(&dir);
        assert!(devices.authenticate("d_1", &token).is_some());
        assert!(devices.authenticate("d_1", "fmd1_wrong").is_none());
        assert!(devices.authenticate("d_2", &token).is_none());
        save_devices(&dir, &devices).unwrap();
        let text = fs::read_to_string(dir.join(DEVICES_FILE)).unwrap();
        assert!(text.contains("\"future\": true"));
        assert!(text.contains("\"extra_field\": \"kept\""));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unparsable_devices_file_is_moved_aside() {
        let dir = temp_dir("bad-devices");
        fs::write(dir.join(DEVICES_FILE), "{not json").unwrap();
        assert!(load_devices(&dir).devices.is_empty());
        assert!(!dir.join(DEVICES_FILE).exists());
        assert!(fs::read_dir(&dir).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("devices.json.bad-")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn device_names_are_trimmed_and_capped() {
        assert_eq!(clean_device_name("  iPhone \n"), "iPhone");
        assert_eq!(clean_device_name(""), "Phone");
        assert_eq!(clean_device_name(&"x".repeat(100)).len(), 64);
        assert_eq!(redact("ExponentPushToken[abcdefgh]"), "…defgh]");
    }
}
