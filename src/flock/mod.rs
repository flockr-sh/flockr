//! Flock: several flockr servers joined through one hub.
//!
//! One machine is the hub: it holds the member list and is the only
//! authority on who is in the flock. Every other member connects only to the
//! hub. Everything here works on files under `state_dir()/flock/` and needs
//! no running server, so the CLI and the server share it. The server's
//! network side is `crate::server::flock`.

pub(crate) mod channel;
pub(crate) mod invite;
pub(crate) mod keys;
pub(crate) mod store;
