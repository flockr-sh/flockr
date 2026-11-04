//! Server-loop handlers for the mobile gateway's internal events.
//!
//! A mobile surface is an ordinary headless client connection in
//! `TerminalObserve` (or `TerminalAttach` for fit mode) mode whose writer
//! stays in process and whose render state yields whole cell frames.

use super::*;
use crate::server::client_transport::ClientWriter;
use crate::server::mobile::{SurfaceOpenError, SurfaceOpened};

/// Lines a desktop mouse wheel notch usually scrolls; one phone wheel event per this many dragged rows.
const WHEEL_LINES_PER_EVENT: i32 = 3;

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
        // Fit surfaces share the size: a new one takes it from another fit
        // surface, which keeps watching. A plain `terminal attach` keeps it.
        if fit && self.terminal_attach_owner_is_exclusive(&terminal_id) {
            return Err(SurfaceOpenError::Busy);
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
            self.demote_mobile_fit_owner(&terminal_id);
            self.attach_terminal_client(client_id, terminal_id.clone(), false)
        } else {
            self.observe_terminal_client(client_id, terminal_id.clone())
        };
        if !opened {
            // The attach/observe path already removed the client on failure.
            return Err(SurfaceOpenError::NotFound);
        }
        self.mobile_surface_devices.insert(client_id, device_id);
        if fit {
            self.mobile_fit_surfaces.insert(client_id);
            if let Some(real_id) = self.terminal_id_by_string(&terminal_id) {
                self.app.state.mobile_fit_holds.insert(real_id);
            }
        }
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

    /// True when `terminal_id` is attached by something other than a fit
    /// surface, which a fit surface must not take the size from.
    fn terminal_attach_owner_is_exclusive(&self, terminal_id: &str) -> bool {
        self.terminal_attach_owners
            .get(terminal_id)
            .is_some_and(|owner| !self.mobile_fit_surfaces.contains(owner))
    }

    /// Moves the fit surface that holds `terminal_id`'s size, if any, to
    /// watching at whatever size the terminal gets next. Returns true when
    /// one was moved.
    pub(super) fn demote_mobile_fit_owner(&mut self, terminal_id: &str) -> bool {
        let Some(&owner) = self.terminal_attach_owners.get(terminal_id) else {
            return false;
        };
        if !self.mobile_fit_surfaces.contains(&owner) {
            return false;
        }
        self.terminal_attach_owners.remove(terminal_id);
        if let Some(real_id) = self.terminal_id_by_string(terminal_id) {
            self.app.state.direct_attach_resize_locks.remove(&real_id);
            self.app.state.mobile_fit_holds.remove(&real_id);
        }
        if let Some(client) = self.clients.get_mut(&owner) {
            client.mode = ClientConnectionMode::TerminalObserve {
                terminal_id: terminal_id.to_owned(),
            };
            client.render_state.request_repaint();
        }
        info!(
            client_id = owner,
            terminal_id, "mobile fit surface gave up the size"
        );
        true
    }

    /// Gives a fit surface its terminal's size: the last surface used holds
    /// it. Does nothing when a plain `terminal attach` holds the terminal.
    pub(super) fn claim_mobile_fit_surface(&mut self, client_id: u64) -> bool {
        if !self.mobile_fit_surfaces.contains(&client_id) {
            return false;
        }
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        let (terminal_id, owner) = match &client.mode {
            ClientConnectionMode::TerminalAttach { terminal_id } => (terminal_id.clone(), true),
            ClientConnectionMode::TerminalObserve { terminal_id } => (terminal_id.clone(), false),
            _ => return false,
        };
        let (cols, rows) = client.terminal_size;
        let cell_size = client.cell_size;
        if !owner {
            if self.terminal_attach_owner_is_exclusive(&terminal_id) {
                return false;
            }
            self.demote_mobile_fit_owner(&terminal_id);
            let Some(real_id) = self.terminal_id_by_string(&terminal_id) else {
                return false;
            };
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.mode = ClientConnectionMode::TerminalAttach {
                    terminal_id: terminal_id.clone(),
                };
                client.render_state.request_repaint();
            }
            self.terminal_attach_owners
                .insert(terminal_id.clone(), client_id);
            self.app
                .state
                .direct_attach_resize_locks
                .insert(real_id.clone());
            self.app.state.mobile_fit_holds.insert(real_id);
            info!(client_id, terminal_id = %terminal_id, cols, rows, "mobile fit surface took the size");
        }
        if let Some(runtime) = self.runtime_for_terminal_id_string(&terminal_id) {
            runtime.resize(rows, cols, cell_size.width_px, cell_size.height_px);
        }
        !owner
    }

    /// A client shell used `tab_id`: fit surfaces give its panes back to the
    /// tab layout. Returns true when any surface gave one up.
    pub(super) fn release_mobile_fit_in_tab(&mut self, tab_id: &str) -> bool {
        if self.mobile_fit_surfaces.is_empty() {
            return false;
        }
        let Some((workspace_index, tab_index)) = self.app.parse_tab_id(tab_id) else {
            return false;
        };
        let Some(tab) = self
            .app
            .state
            .workspaces
            .get(workspace_index)
            .and_then(|workspace| workspace.tabs.get(tab_index))
        else {
            return false;
        };
        let terminals = tab
            .panes
            .values()
            .map(|pane| pane.attached_terminal_id.to_string())
            .collect::<Vec<_>>();
        let mut released = false;
        for terminal_id in terminals {
            released |= self.demote_mobile_fit_owner(&terminal_id);
        }
        released
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
        apply_client_pane_input_events(runtime, events)?;
        if let Some(terminal_id) = self.terminal_id_by_string(&terminal_id) {
            self.note_person_input(&terminal_id);
        }
        Ok(())
    }

    pub(super) fn apply_mobile_pane_wheel(
        &mut self,
        target: &str,
        lines: i32,
        column: Option<u16>,
        row: Option<u16>,
    ) -> Result<bool, String> {
        if self.handoff_in_progress {
            return Err("server is handing off".to_owned());
        }
        let terminal_id = self
            .resolve_terminal_target_id_string(target)
            .ok_or_else(|| format!("terminal target {target} not found"))?;
        let runtime = self
            .runtime_for_terminal_id_string(&terminal_id)
            .ok_or_else(|| format!("terminal {terminal_id} not found"))?;
        if !matches!(
            runtime.wheel_routing(),
            Some(
                crate::pane::WheelRouting::MouseReport | crate::pane::WheelRouting::AlternateScroll
            )
        ) {
            return Ok(false);
        }
        // Apps scroll several lines per wheel event, so a drag of `lines` rows
        // sends one event per WHEEL_LINES_PER_EVENT and carries the rest over.
        let pending = self
            .mobile_wheel_remainder
            .entry(terminal_id.clone())
            .or_default();
        let total = *pending + lines;
        let events = total / WHEEL_LINES_PER_EVENT;
        *pending = total - events * WHEEL_LINES_PER_EVENT;
        if events == 0 {
            return Ok(true);
        }
        let runtime = self
            .runtime_for_terminal_id_string(&terminal_id)
            .ok_or_else(|| format!("terminal {terminal_id} not found"))?;
        let (rows, cols) = runtime.current_size();
        let column = column.unwrap_or(cols / 2).min(cols.saturating_sub(1));
        let row = row.unwrap_or(rows / 2).min(rows.saturating_sub(1));
        let direction = if events > 0 {
            protocol::AttachScrollDirection::Up
        } else {
            protocol::AttachScrollDirection::Down
        };
        for _ in 0..events.unsigned_abs() {
            apply_terminal_attach_scroll(
                runtime,
                protocol::AttachScrollSource::Wheel,
                direction,
                1,
                Some(column),
                Some(row),
                0,
            )?;
        }
        Ok(true)
    }
}
