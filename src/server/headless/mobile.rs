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

/// Which panes a client shell takes back from phone and web fit surfaces
/// when it claims its tab's geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FitRelease {
    /// None: the client only focused its window.
    None,
    /// Every pane in the tab: the client opened or switched to it.
    Tab,
    /// One terminal: the client typed or clicked in that pane.
    Terminal(String),
}

impl HeadlessServer {
    pub(super) fn open_mobile_surface(
        &mut self,
        client_id: u64,
        device_id: String,
        target: String,
        fit: bool,
        resume: bool,
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

        // Opening takes the size. A subscription restored after a reconnect
        // takes it only if this device held it last; otherwise it watches
        // until the device is used.
        let take = fit && (!resume || self.mobile_fit_device_held_last(&terminal_id, &device_id));
        let opened = if take {
            self.demote_mobile_fit_owner(&terminal_id);
            self.mobile_fit_last_device.remove(&terminal_id);
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
        }
        if take {
            if let Some(real_id) = self.terminal_id_by_string(&terminal_id) {
                self.app.state.mobile_fit_holds.insert(real_id);
            }
        }
        let (rows, cols) = self
            .runtime_for_terminal_id_string(&terminal_id)
            .map(|runtime| runtime.current_size())
            .unwrap_or((rows, cols));
        info!(client_id, terminal_id = %terminal_id, fit, resume, take, "mobile surface opened");
        Ok(SurfaceOpened {
            pane_id,
            terminal_id,
            cols,
            rows,
        })
    }

    /// True when `device_id` held `terminal_id`'s size last: its fit surface
    /// still holds it (a half-open old connection), or held it when its
    /// connection dropped and nobody has used the pane since.
    fn mobile_fit_device_held_last(&self, terminal_id: &str, device_id: &str) -> bool {
        match self.terminal_attach_owners.get(terminal_id) {
            Some(owner) => {
                self.mobile_fit_surfaces.contains(owner)
                    && self.mobile_surface_devices.get(owner).map(String::as_str) == Some(device_id)
            }
            None => {
                self.mobile_fit_last_device
                    .get(terminal_id)
                    .map(String::as_str)
                    == Some(device_id)
            }
        }
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
            self.mobile_fit_last_device.remove(&terminal_id);
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

    /// Takes panes back from fit surfaces for a client shell, as `release`
    /// says. Returns true when any surface gave one up.
    pub(super) fn release_mobile_fit(&mut self, tab_id: &str, release: &FitRelease) -> bool {
        match release {
            FitRelease::None => false,
            FitRelease::Tab => self.release_mobile_fit_in_tab(tab_id),
            FitRelease::Terminal(terminal_id) => self.release_mobile_fit_terminal(terminal_id),
        }
    }

    /// A client shell used one pane: its fit surface, if any, gives the size
    /// back to the tab layout.
    fn release_mobile_fit_terminal(&mut self, terminal_id: &str) -> bool {
        self.mobile_fit_last_device.remove(terminal_id);
        self.demote_mobile_fit_owner(terminal_id)
    }

    /// The fit release for a client shell using `pane_id` in `ws_idx`.
    pub(super) fn fit_release_for_pane(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> FitRelease {
        self.app
            .state
            .terminal_id_for_pane(ws_idx, pane_id)
            .map_or(FitRelease::None, |id| FitRelease::Terminal(id.to_string()))
    }

    /// A client shell opened `tab_id`: fit surfaces give its panes back to the
    /// tab layout. Returns true when any surface gave one up.
    fn release_mobile_fit_in_tab(&mut self, tab_id: &str) -> bool {
        if self.mobile_fit_surfaces.is_empty() && self.mobile_fit_last_device.is_empty() {
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
            released |= self.release_mobile_fit_terminal(&terminal_id);
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
