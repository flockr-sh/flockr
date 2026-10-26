//! Pairing payload (QR / URL) and bootstrap-code redemption.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::store::{self, DeviceRecord, PairingRecord};

pub(crate) const PAIR_URL_PREFIX: &str = "flockr://pair?p=";

/// JSON inside the pairing URL. Keys match `docs/mobile/protocol-v1.md` §6.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PairingPayload {
    pub v: u32,
    pub name: String,
    pub hosts: Vec<String>,
    pub port: u16,
    pub sid: String,
    pub code: String,
    pub exp: u64,
}

impl PairingPayload {
    pub(crate) fn to_url(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        format!("{PAIR_URL_PREFIX}{}", store::base64url(&json))
    }

    #[cfg(test)]
    pub(crate) fn from_url(url: &str) -> Result<Self, String> {
        let encoded = url
            .trim()
            .strip_prefix(PAIR_URL_PREFIX)
            .ok_or_else(|| "not a flockr pairing URL".to_owned())?;
        let encoded = encoded.split('&').next().unwrap_or(encoded);
        use base64::Engine as _;
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|err| format!("invalid pairing payload: {err}"))?;
        serde_json::from_slice(&json).map_err(|err| format!("invalid pairing payload: {err}"))
    }
}

/// Renders `text` as a QR code with Unicode half blocks (error correction M).
pub(crate) fn render_qr(text: &str) -> Result<String, String> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .map_err(|err| format!("could not build QR code: {err}"))?;
    Ok(code
        .render::<qrcode::render::unicode::Dense1x2>()
        .dark_color(qrcode::render::unicode::Dense1x2::Light)
        .light_color(qrcode::render::unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

/// Why redeeming a bootstrap code failed. The app only sees the error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PairError {
    /// No live code, wrong code, wrong server, expired, or too many failures.
    Failed(&'static str),
    TooManyDevices,
    Io(String),
}

/// Inputs from the app's `hello.pair`.
pub(crate) struct PairRequest<'a> {
    pub code: &'a str,
    pub sid: &'a str,
    pub device_name: &'a str,
    pub platform: &'a str,
}

/// A newly paired device and its one-time plain token.
#[derive(Debug)]
pub(crate) struct Paired {
    pub device: DeviceRecord,
    pub token: String,
}

/// Redeems the pending bootstrap code in `dir` and creates a device record.
///
/// Callers serialize calls (one pairing at a time per process).
pub(crate) fn redeem(
    dir: &Path,
    request: &PairRequest<'_>,
    server_id: &str,
    max_devices: u32,
    now: u64,
) -> Result<Paired, PairError> {
    let io_err = |err: io::Error| PairError::Io(err.to_string());
    let mut pairing = match store::load_pairing(dir) {
        Ok(Some(pairing)) => pairing,
        Ok(None) => return Err(PairError::Failed("no pending pairing code")),
        Err(err) => return Err(io_err(err)),
    };
    if !pairing.is_live(now) {
        return Err(PairError::Failed("pairing code expired or used"));
    }
    let sid_ok = store::constant_time_eq(request.sid.as_bytes(), server_id.as_bytes());
    let code_ok = store::secret_matches(request.code, &pairing.code_sha256);
    if !sid_ok || !code_ok {
        pairing.failed_attempts = pairing.failed_attempts.saturating_add(1);
        store::save_pairing(dir, &pairing).map_err(io_err)?;
        return Err(PairError::Failed(if sid_ok {
            "wrong pairing code"
        } else {
            "pairing code is for another server"
        }));
    }
    let devices = store::load_devices(dir);
    if devices.devices.len() >= max_devices as usize {
        return Err(PairError::TooManyDevices);
    }
    let token = store::generate_device_token().map_err(io_err)?;
    let device = DeviceRecord {
        device_id: store::generate_device_id().map_err(io_err)?,
        name: store::clean_device_name(request.device_name),
        platform: request.platform.chars().take(32).collect(),
        token_sha256: store::sha256_hex(&token),
        created_at: now,
        last_seen_at: now,
        push: None,
        extra: serde_json::Map::new(),
    };
    store::update_devices(dir, |devices| devices.devices.push(device.clone())).map_err(io_err)?;
    pairing.consumed_by = Some(device.device_id.clone());
    store::save_pairing(dir, &pairing).map_err(io_err)?;
    Ok(Paired { device, token })
}

/// Writes a fresh pairing code and returns `(code, record)`.
pub(crate) fn create_pairing(
    dir: &Path,
    ttl_secs: u64,
    now: u64,
) -> io::Result<(String, PairingRecord)> {
    let code = store::generate_pairing_code()?;
    let record = PairingRecord::new(&code, now, ttl_secs);
    store::save_pairing(dir, &record)?;
    Ok((code, record))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::store::tests::temp_dir;

    fn request<'a>(code: &'a str, sid: &'a str) -> PairRequest<'a> {
        PairRequest {
            code,
            sid,
            device_name: " Dylan's iPhone ",
            platform: "ios",
        }
    }

    #[test]
    fn payload_round_trips_through_the_url() {
        let payload = PairingPayload {
            v: 1,
            name: "dk-desktop".into(),
            hosts: vec!["100.92.14.3".into(), "dk.netbird.cloud".into()],
            port: 47390,
            sid: "Xb3k9QmT0aLz7Rwe2YvN1c".into(),
            code: "q1W2e3R4t5Y6u7I8o9P0aA".into(),
            exp: 1761330600,
        };
        let url = payload.to_url();
        assert!(url.starts_with("flockr://pair?p="));
        assert!(!url.ends_with('='));
        assert_eq!(PairingPayload::from_url(&url).unwrap(), payload);
        assert!(PairingPayload::from_url("https://example.com").is_err());
        assert!(render_qr(&url).unwrap().contains('█'));
    }

    #[test]
    fn code_is_single_use_and_issues_a_hashed_token() {
        let dir = temp_dir("pair-ok");
        let (code, _) = create_pairing(&dir, 600, 1000).unwrap();
        let paired = redeem(&dir, &request(&code, "sid"), "sid", 8, 1001).unwrap();
        assert_eq!(paired.device.name, "Dylan's iPhone");
        assert!(paired.token.starts_with("fmd1_"));
        let devices = store::load_devices(&dir);
        assert!(devices
            .authenticate(&paired.device.device_id, &paired.token)
            .is_some());
        assert!(!std::fs::read_to_string(dir.join(store::DEVICES_FILE))
            .unwrap()
            .contains(&paired.token));
        let pairing = store::load_pairing(&dir).unwrap().unwrap();
        assert_eq!(
            pairing.consumed_by.as_deref(),
            Some(paired.device.device_id.as_str())
        );
        assert!(matches!(
            redeem(&dir, &request(&code, "sid"), "sid", 8, 1002),
            Err(PairError::Failed(_))
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn expired_wrong_and_foreign_codes_fail() {
        let dir = temp_dir("pair-fail");
        let (code, _) = create_pairing(&dir, 60, 1000).unwrap();
        assert!(matches!(
            redeem(&dir, &request(&code, "sid"), "sid", 8, 1060),
            Err(PairError::Failed(_))
        ));
        let (code, _) = create_pairing(&dir, 60, 2000).unwrap();
        assert!(matches!(
            redeem(&dir, &request(&code, "other"), "sid", 8, 2001),
            Err(PairError::Failed("pairing code is for another server"))
        ));
        for _ in 0..4 {
            assert!(redeem(&dir, &request("wrong", "sid"), "sid", 8, 2001).is_err());
        }
        // Five failures kill the code even for the right value.
        assert!(matches!(
            redeem(&dir, &request(&code, "sid"), "sid", 8, 2001),
            Err(PairError::Failed("pairing code expired or used"))
        ));
        assert!(store::load_devices(&dir).devices.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pairing_stops_at_max_devices_and_revoke_frees_a_slot() {
        let dir = temp_dir("pair-max");
        let (code, _) = create_pairing(&dir, 600, 1000).unwrap();
        let first = redeem(&dir, &request(&code, "sid"), "sid", 1, 1001).unwrap();
        let (code, _) = create_pairing(&dir, 600, 1000).unwrap();
        assert_eq!(
            redeem(&dir, &request(&code, "sid"), "sid", 1, 1001).unwrap_err(),
            PairError::TooManyDevices
        );
        store::update_devices(&dir, |devices| devices.remove(&first.device.device_id)).unwrap();
        assert!(store::load_devices(&dir)
            .authenticate(&first.device.device_id, &first.token)
            .is_none());
        assert!(redeem(&dir, &request(&code, "sid"), "sid", 1, 1002).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
