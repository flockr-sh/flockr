//! Mobile gateway state shared by the CLI (`flockr mobile ...`) and the server.
//!
//! Everything here works on files under `state_dir()/mobile/` and needs no
//! running server. The wire contract is `docs/mobile/protocol-v1.md`.

pub(crate) mod pairing;
pub(crate) mod store;
