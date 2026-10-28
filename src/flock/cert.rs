//! Membership certificates: short-lived proof that a member is in the group.
//! Members present one on every connection, so a removed device loses access
//! to members that missed the removal once its certificate lapses.

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use super::keys::{self, encode_key};
use super::log::State;

pub(crate) const LIFETIME_SECS: u64 = 24 * 60 * 60;
/// Renew when less than this much time is left.
pub(crate) const RENEW_BEFORE_SECS: u64 = 23 * 60 * 60;
const SIGNING_CONTEXT: &[u8] = b"flockr.flock.cert.v1\0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CertBody {
    pub group: String,
    pub member: String,
    pub noise_key: String,
    pub expires: u64,
    /// Signing key of the owner or admin that issued it.
    pub issuer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cert {
    pub body: String,
    pub sig: String,
}

fn signed_bytes(body: &str) -> Vec<u8> {
    let mut bytes = SIGNING_CONTEXT.to_vec();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

/// Issues a certificate for `member`. `None` when the member isn't active.
pub(crate) fn issue(state: &State, member: &str, issuer: &SigningKey, now: u64) -> Option<Cert> {
    let target = state.active(member)?;
    let body = CertBody {
        group: state.group.clone(),
        member: member.to_owned(),
        noise_key: target.info.noise_key.clone(),
        expires: now + LIFETIME_SECS,
        issuer: encode_key(&issuer.verifying_key().to_bytes()),
    };
    let body = serde_json::to_string(&body).ok()?;
    let sig = keys::sign(issuer, &signed_bytes(&body));
    Some(Cert { body, sig })
}

impl Cert {
    pub(crate) fn parsed(&self) -> Option<CertBody> {
        serde_json::from_str(&self.body).ok()
    }

    /// Checks the certificate for the member using `noise_key`. Returns the
    /// member ID.
    pub(crate) fn verify(
        &self,
        state: &State,
        noise_key: &str,
        now: u64,
    ) -> Result<String, &'static str> {
        let body = self.parsed().ok_or("certificate is malformed")?;
        if !keys::verify(&body.issuer, &signed_bytes(&self.body), &self.sig) {
            return Err("certificate signature is invalid");
        }
        if body.group != state.group {
            return Err("certificate is for another group");
        }
        if !state.is_admin_key(&body.issuer) {
            return Err("certificate issuer is not an admin");
        }
        let member = state
            .active(&body.member)
            .ok_or("member is not in the group")?;
        if member.info.noise_key != noise_key || body.noise_key != noise_key {
            return Err("certificate is for another key");
        }
        if body.expires <= now {
            return Err("certificate has expired");
        }
        Ok(body.member)
    }

    pub(crate) fn needs_renewal(&self, now: u64) -> bool {
        self.parsed()
            .is_none_or(|body| body.expires.saturating_sub(now) < RENEW_BEFORE_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flock::keys::MemberKeys;
    use crate::flock::log::tests::{info, owner};
    use crate::flock::log::{fold, genesis, Op, Role};

    #[test]
    fn certificates_check_issuer_member_key_and_expiry() {
        let owner = owner();
        let mut log = vec![genesis("home", &owner, 0)];
        let admin = MemberKeys::generate().unwrap();
        let laptop = MemberKeys::generate().unwrap();
        for (keys, role) in [(&admin, Role::Admin), (&laptop, Role::Member)] {
            let state = fold(&log, None).unwrap();
            log.push(state.append(
                Op::Add {
                    member: info(keys, "m"),
                    role,
                },
                &owner,
                1,
            ));
        }
        let state = fold(&log, None).unwrap();
        let noise = encode_key(&laptop.noise_public().unwrap());
        let cert = issue(&state, &laptop.member_id(), &admin.sign, 100).unwrap();
        assert_eq!(cert.verify(&state, &noise, 101), Ok(laptop.member_id()));
        assert!(cert.verify(&state, &noise, 100 + LIFETIME_SECS).is_err());
        let other_noise = encode_key(&admin.noise_public().unwrap());
        assert!(cert.verify(&state, &other_noise, 101).is_err());
        assert!(!cert.needs_renewal(101));
        assert!(cert.needs_renewal(100 + LIFETIME_SECS - RENEW_BEFORE_SECS + 1));

        // A member can't issue certificates.
        let forged = issue(&state, &laptop.member_id(), &laptop.sign, 100).unwrap();
        assert!(forged.verify(&state, &noise, 101).is_err());

        // Removal invalidates a live certificate.
        log.push(state.append(
            Op::Remove {
                member: laptop.member_id(),
            },
            &owner,
            2,
        ));
        let state = fold(&log, None).unwrap();
        assert!(cert.verify(&state, &noise, 101).is_err());
        assert!(issue(&state, &laptop.member_id(), &admin.sign, 100).is_none());
    }
}
