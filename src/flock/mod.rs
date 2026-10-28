//! Flock: several flockr servers joined as one group.
//!
//! Everything here works on files under `state_dir()/flock/` and needs no
//! running server, so the CLI and the server share it. The server's network
//! side is `crate::server::flock`.

pub(crate) mod channel;
pub(crate) mod invite;
pub(crate) mod keys;
pub(crate) mod log;
pub(crate) mod owner;
pub(crate) mod store;
