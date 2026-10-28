//! Invites and join requests.
//!
//! `flockr flock invite` writes an invite; the server redeems it when a new
//! machine connects and writes a join request; the CLI shows the request and
//! writes the owner's decision; the server sends the result to the new
//! machine. Each step is its own file, so no file has two writers.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::log::{MemberInfo, Role};
use crate::mobile::store::{
    atomic_create, atomic_write, base64url, random_bytes, secret_matches, sha256_hex,
};

pub(crate) const JOIN_URL_PREFIX: &str = "flockr://join?p=";
pub(crate) const PAYLOAD_VERSION: u32 = 1;
pub(crate) const DEFAULT_TTL_SECS: u64 = 600;
pub(crate) const INVITES_DIR: &str = "invites";
pub(crate) const REQUESTS_DIR: &str = "requests";

/// JSON inside the join URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InvitePayload {
    pub v: u32,
    pub group: String,
    pub name: String,
    /// Member that issued the invite and accepts the join.
    pub host: String,
    /// The host's Noise public key.
    pub noise_key: String,
    pub addrs: Vec<String>,
    pub invite: String,
    pub secret: String,
    pub exp: u64,
}

impl InvitePayload {
    pub(crate) fn to_url(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        format!("{JOIN_URL_PREFIX}{}", base64url(&json))
    }

    pub(crate) fn from_url(url: &str) -> Result<Self, String> {
        use base64::Engine as _;
        let encoded = url
            .trim()
            .strip_prefix(JOIN_URL_PREFIX)
            .ok_or_else(|| "not a flockr join URL".to_owned())?;
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|err| format!("invalid join URL: {err}"))?;
        let payload: Self =
            serde_json::from_slice(&json).map_err(|err| format!("invalid join URL: {err}"))?;
        if payload.v != PAYLOAD_VERSION {
            return Err(format!(
                "join URL version {} is not supported; update flockr",
                payload.v
            ));
        }
        Ok(payload)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct InviteRecord {
    pub version: u32,
    pub id: String,
    pub secret_sha256: String,
    pub role: Role,
    pub expires_at: u64,
}

fn invite_path(dir: &Path, id: &str, ext: &str) -> std::path::PathBuf {
    dir.join(INVITES_DIR).join(format!("{id}.{ext}"))
}

fn request_path(dir: &Path, id: &str, ext: &str) -> std::path::PathBuf {
    dir.join(REQUESTS_DIR).join(format!("{id}.{ext}"))
}

/// IDs are generated here, but redemption takes them from the network.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// Creates an invite and returns it with its one-time secret.
pub(crate) fn create(
    dir: &Path,
    role: Role,
    ttl: u64,
    now: u64,
) -> io::Result<(InviteRecord, String)> {
    let id = format!("i_{}", base64url(&random_bytes::<9>()?));
    let secret = base64url(&random_bytes::<16>()?);
    let record = InviteRecord {
        version: 1,
        id: id.clone(),
        secret_sha256: sha256_hex(&secret),
        role,
        expires_at: now + ttl,
    };
    let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
    atomic_write(&invite_path(dir, &id, "json"), &bytes)?;
    Ok((record, secret))
}

pub(crate) fn load(dir: &Path, id: &str) -> Option<InviteRecord> {
    if !valid_id(id) {
        return None;
    }
    let text = fs::read_to_string(invite_path(dir, id, "json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Checks the secret and marks the invite used. Single use: a second
/// redemption fails even with the right secret.
pub(crate) fn redeem(
    dir: &Path,
    id: &str,
    secret: &str,
    now: u64,
) -> Result<InviteRecord, &'static str> {
    let record = load(dir, id).ok_or("no such invite")?;
    if record.expires_at <= now {
        return Err("invite expired");
    }
    if !secret_matches(secret, &record.secret_sha256) {
        return Err("wrong invite secret");
    }
    match atomic_create(&invite_path(dir, id, "used"), b"") {
        Ok(true) => Ok(record),
        Ok(false) => Err("invite already used"),
        Err(_) => Err("could not record the invite as used"),
    }
}

/// A machine waiting for the owner to approve its join.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JoinRequest {
    pub version: u32,
    pub invite: String,
    pub role: Role,
    pub member: MemberInfo,
    /// Short code both screens show.
    pub sas: String,
    pub at: u64,
}

pub(crate) fn save_request(dir: &Path, request: &JoinRequest) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(request).map_err(io::Error::other)?;
    atomic_write(&request_path(dir, &request.invite, "json"), &bytes)
}

pub(crate) fn load_request(dir: &Path, invite: &str) -> Option<JoinRequest> {
    if !valid_id(invite) {
        return None;
    }
    let text = fs::read_to_string(request_path(dir, invite, "json")).ok()?;
    serde_json::from_str(&text).ok()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub(crate) enum Decision {
    Approved,
    Denied,
}

pub(crate) fn save_decision(dir: &Path, invite: &str, decision: &Decision) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(decision).map_err(io::Error::other)?;
    atomic_write(&request_path(dir, invite, "decision"), &bytes)
}

pub(crate) fn load_decision(dir: &Path, invite: &str) -> Option<Decision> {
    if !valid_id(invite) {
        return None;
    }
    let text = fs::read_to_string(request_path(dir, invite, "decision")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Removes an invite and its request files.
pub(crate) fn clear(dir: &Path, id: &str) {
    if !valid_id(id) {
        return;
    }
    for ext in ["json", "used"] {
        let _ = fs::remove_file(invite_path(dir, id, ext));
    }
    for ext in ["json", "decision"] {
        let _ = fs::remove_file(request_path(dir, id, ext));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invites_redeem_once_with_the_right_secret_before_expiry() {
        let dir = crate::mobile::store::tests::temp_dir("flock-invite");
        let (record, secret) = create(&dir, Role::Admin, 60, 1000).unwrap();
        assert_eq!(
            redeem(&dir, &record.id, "wrong", 1001).unwrap_err(),
            "wrong invite secret"
        );
        assert_eq!(
            redeem(&dir, &record.id, &secret, 1060).unwrap_err(),
            "invite expired"
        );
        assert_eq!(
            redeem(&dir, &record.id, &secret, 1001).unwrap().role,
            Role::Admin
        );
        assert_eq!(
            redeem(&dir, &record.id, &secret, 1001).unwrap_err(),
            "invite already used"
        );
        assert_eq!(
            redeem(&dir, "../x", &secret, 1001).unwrap_err(),
            "no such invite"
        );
    }

    #[test]
    fn join_urls_round_trip() {
        let payload = InvitePayload {
            v: PAYLOAD_VERSION,
            group: "g".into(),
            name: "home".into(),
            host: "m_x".into(),
            noise_key: "k".into(),
            addrs: vec!["10.0.0.1:47392".into()],
            invite: "i_1".into(),
            secret: "s".into(),
            exp: 5,
        };
        assert_eq!(InvitePayload::from_url(&payload.to_url()).unwrap(), payload);
        assert!(InvitePayload::from_url("flockr://pair?p=x").is_err());
    }
}
