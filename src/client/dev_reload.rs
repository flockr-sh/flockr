//! Dev-only support for `just reload` (cargo feature `dev-reload`).
//!
//! When the server hands off to a rebuilt binary, the TUI execs that binary in
//! place instead of exiting, and the new process waits for the new server.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use tracing::{debug, info};

use super::{ClientError, TerminalGuard};
use crate::ipc::LocalStream;

/// Set on the exec'd client so it waits for the new server instead of failing.
const RESUME_ENV_VAR: &str = "FLOCKR_DEV_RELOAD_RESUME";
const RESUME_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_INTERVAL: Duration = Duration::from_millis(100);
/// Must match the reason the server sends in `disconnect_all_clients_for_handoff`.
const HANDOFF_SHUTDOWN_REASON: &str = "live update in progress; reconnect after handoff completes";

/// Reads and clears the resume marker so nothing started later inherits it.
pub(super) fn take_resume_request() -> bool {
    let resume = std::env::var_os(RESUME_ENV_VAR).is_some();
    if resume {
        std::env::remove_var(RESUME_ENV_VAR);
    }
    resume
}

pub(super) fn is_handoff_shutdown(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::ServerShutdown { reason: Some(reason) } if reason == HANDOFF_SHUTDOWN_REASON
    )
}

/// Replaces this process with the rebuilt client. Returns only on failure.
pub(super) fn exec_rebuilt_client(terminal_guard: &mut TerminalGuard) -> io::Error {
    use std::os::unix::process::CommandExt as _;

    let exe = match crate::platform::launch_executable() {
        Ok(exe) => exe,
        Err(error) => return error,
    };
    terminal_guard.release_for_exec();
    info!(exe = %exe.display(), "dev reload: exec rebuilt client");
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
        Err(error) => debug!(%error, "dev reload: server not ready"),
    }
    let deadline = Instant::now() + RESUME_TIMEOUT;
    loop {
        std::thread::sleep(RETRY_INTERVAL);
        match crate::ipc::connect_local_stream(socket_path).and_then(&mut attach) {
            Ok(attached) => return Ok(Some(attached)),
            Err(error) if Instant::now() < deadline => {
                debug!(%error, "dev reload: server not ready");
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_handoff_reason_triggers_reload() {
        assert!(is_handoff_shutdown(&ClientError::ServerShutdown {
            reason: Some(HANDOFF_SHUTDOWN_REASON.to_owned()),
        }));
        assert!(!is_handoff_shutdown(&ClientError::ServerShutdown {
            reason: Some("detached".to_owned()),
        }));
        assert!(!is_handoff_shutdown(&ClientError::ServerShutdown {
            reason: None
        }));
    }
}
