//! Server-loop handlers for the mobile gateway's internal events.
//!
//! A mobile surface is an ordinary headless client connection in
//! `TerminalObserve` (or `TerminalAttach` for fit mode) mode whose writer
//! stays in process and whose render state yields whole cell frames.

use super::*;
use crate::server::client_transport::ClientWriter;
use crate::server::mobile::{SurfaceOpenError, SurfaceOpened};

impl HeadlessServer {
    pub(super) fn open_mobile_surface(
        &mut self,
        client_id: u64,
        device_id: String,
        target: String,
        fit: bool,
        cols: u16,
        rows: u16,
        writer: ClientWriter,
    ) -> Result<SurfaceOpened, SurfaceOpenError> {
        if self.handoff_in_progress || self.shutting_down {
            return Err(SurfaceOpenError::Unavailable);
        }
        let resolved = self
            .app
            .resolve_terminal_target(&target)
            .map_err(|_| SurfaceOpenError::NotFound)?;
        let terminal_id = resolved.terminal_id.clone();
        let pane_id = self
            .app
            .public_pane_id(resolved.ws_idx, resolved.pane_id)
            .ok_or(SurfaceOpenError::NotFound)?;
        let clients = &self.clients;
        self.mobile_surface_devices
            .retain(|id, _| clients.contains_key(id));
        // A phone that reconnects (network switch, half-open socket) may
        // still own the attach through its old connection; let the same
        // device take it over. Anyone else gets `pane_busy`.
        let mut takeover = false;
        if fit {
            if let Some(owner) = self.terminal_attach_owners.get(&terminal_id) {
                if self.mobile_surface_devices.get(owner) != Some(&device_id) {
                    return Err(SurfaceOpenError::Busy);
                }
                takeover = true;
            }
        }
        if self.clients.contains_key(&client_id) {
            return Err(SurfaceOpenError::Unavailable);
        }

        let last_activity = self.allocate_activity_stamp();
        let mut connection = ClientConnection::new_with_mode(
            ClientConnectionMode::TerminalPending,
            (cols.max(1), rows.max(1)),
            crate::kitty_graphics::HostCellSize::default(),
            last_activity,
            protocol::RenderEncoding::TerminalAnsi,
            Some(writer),
        );
        connection.render_state = crate::server::render_stream::ClientRenderState::cells();
        self.clients.insert(client_id, connection);

        let opened = if fit {
            self.attach_terminal_client(client_id, terminal_id.clone(), takeover)
        } else {
            self.observe_terminal_client(client_id, terminal_id.clone())
        };
        if !opened {
            // The attach/observe path already removed the client on failure.
            return Err(SurfaceOpenError::NotFound);
        }
        self.mobile_surface_devices.insert(client_id, device_id);
        let (rows, cols) = self
            .runtime_for_terminal_id_string(&terminal_id)
            .map(|runtime| runtime.current_size())
            .unwrap_or((rows, cols));
        info!(client_id, terminal_id = %terminal_id, fit, "mobile surface opened");
        Ok(SurfaceOpened {
            pane_id,
            terminal_id,
            cols,
            rows,
        })
    }

    pub(super) fn apply_mobile_pane_input(
        &mut self,
        target: &str,
        events: &[protocol::ClientPaneInputEvent],
    ) -> Result<(), String> {
        if self.handoff_in_progress {
            return Err("server is handing off".to_owned());
        }
        let terminal_id = self
            .resolve_terminal_target_id_string(target)
            .ok_or_else(|| format!("terminal target {target} not found"))?;
        let runtime = self
            .runtime_for_terminal_id_string(&terminal_id)
            .ok_or_else(|| format!("terminal {terminal_id} not found"))?;
        runtime.scroll_reset();
        apply_client_pane_input_events(runtime, events).map(|_| ())
    }
}
