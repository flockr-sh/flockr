//! Keeps the TUI attached across a server live handoff.
//!
//! When the server hands off to a replacement process, it disconnects clients
//! with `LIVE_HANDOFF_SHUTDOWN_REASON`. The client then execs the installed
//! binary in place instead of exiting, and the new process waits for the new
//! server to accept it.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use super::{ClientError, TerminalGuard};
use crate::ipc::LocalStream;
use crate::server::handoff::LIVE_HANDOFF_SHUTDOWN_REASON;

/// Set on the exec'd client so it waits for the new server instead of failing.
/// Keeps its `just reload`-era name so clients exec'd by older builds resume.
const RESUME_ENV_VAR: &str = "FLOCKR_DEV_RELOAD_RESUME";
const RESUME_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Reads and clears the resume marker so nothing started later inherits it.
pub(super) fn take_resume_request() -> bool {
    let resume = std::env::var_os(RESUME_ENV_VAR).is_some();
    if resume {
        std::env::remove_var(RESUME_ENV_VAR);
    }
    resume
}

/// Whether a client that ended with `error` should exec and reattach. Direct
/// attach, federated, and remote clients keep their old exit behavior.
pub(super) fn should_resume(
    error: &ClientError,
    direct_attach: bool,
    federated: bool,
    remote_client_process: bool,
) -> bool {
    is_handoff_shutdown(error) && !direct_attach && !federated && !remote_client_process
}

fn is_handoff_shutdown(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::ServerShutdown { reason: Some(reason) } if reason == LIVE_HANDOFF_SHUTDOWN_REASON
    )
}

/// Replaces this process with the installed client. Returns only on failure.
pub(super) fn exec_client(terminal_guard: &mut TerminalGuard) -> io::Error {
    use std::os::unix::process::CommandExt as _;

    let exe = match crate::platform::launch_executable() {
        Ok(exe) => exe,
        Err(error) => return error,
    };
    terminal_guard.release_for_exec();
    info!(exe = %exe.display(), "live handoff: exec client to reattach");
    std::process::Command::new(exe)
        .arg("client")
        .env(RESUME_ENV_VAR, "1")
        .exec()
}

/// Retries connect and handshake until the new server accepts the client.
pub(super) fn retry_until_attached<T>(
    first: io::Result<Option<T>>,
    socket_path: &Path,
    mut attach: impl FnMut(LocalStream) -> io::Result<T>,
) -> io::Result<Option<T>> {
    match first {
        Ok(Some(attached)) => return Ok(Some(attached)),
        Ok(None) => {}
        Err(error) => debug!(%error, "live handoff: server not ready"),
    }
    let deadline = Instant::now() + RESUME_TIMEOUT;
    loop {
        std::thread::sleep(RETRY_INTERVAL);
        match crate::ipc::connect_local_stream(socket_path).and_then(&mut attach) {
            Ok(attached) => return Ok(Some(attached)),
            Err(error) if Instant::now() < deadline => {
                debug!(%error, "live handoff: server not ready");
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handoff_shutdown() -> ClientError {
        ClientError::ServerShutdown {
            reason: Some(LIVE_HANDOFF_SHUTDOWN_REASON.to_owned()),
        }
    }

    #[test]
    fn only_the_handoff_reason_triggers_resume() {
        assert!(should_resume(&handoff_shutdown(), false, false, false));
        assert!(!should_resume(
            &ClientError::ServerShutdown {
                reason: Some("detached".to_owned()),
            },
            false,
            false,
            false
        ));
        assert!(!should_resume(
            &ClientError::ServerShutdown {
                reason: Some("server is shutting down".to_owned()),
            },
            false,
            false,
            false
        ));
        assert!(!should_resume(
            &ClientError::ServerShutdown { reason: None },
            false,
            false,
            false
        ));
    }

    #[test]
    fn direct_attach_federated_and_remote_clients_do_not_resume() {
        assert!(!should_resume(&handoff_shutdown(), true, false, false));
        assert!(!should_resume(&handoff_shutdown(), false, true, false));
        assert!(!should_resume(&handoff_shutdown(), false, false, true));
    }
}
