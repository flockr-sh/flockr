use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

pub(super) fn normalized_theme_name(name: &str) -> String {
    name.to_lowercase().replace([' ', '_'], "-")
}

pub(super) fn integration_needs_install(info: &crate::api::schema::IntegrationInfo) -> bool {
    info.state == crate::api::schema::IntegrationState::Outdated
        || info.available && info.state == crate::api::schema::IntegrationState::NotInstalled
}

/// Read the config file for the settings rows. A missing file reads as the
/// defaults; an unreadable one reads as the defaults plus the reason.
fn load_settings_config() -> (Box<crate::config::Config>, Option<String>) {
    match crate::config::load_live_config() {
        Ok(loaded) => (Box::new(loaded.config), None),
        Err(errors) => (Box::default(), Some(errors.join("; "))),
    }
}

/// The config path with the home directory shortened to `~`.
fn display_config_path() -> String {
    let path = crate::config::config_path();
    match std::env::var_os("HOME").and_then(|home| {
        path.strip_prefix(home)
            .ok()
            .map(|rest| rest.display().to_string())
    }) {
        Some(rest) => format!("~/{rest}"),
        None => path.display().to_string(),
    }
}

impl ClientShellState {
    pub(super) fn open_settings_overlay(&mut self) {
        let (config, config_error) = load_settings_config();
        self.overlay = Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            section: ClientSettingsSection::Appearance,
            selected: 0,
            config,
            config_error,
            config_path: display_config_path(),
            integrations: Vec::new(),
            integration_messages: Vec::new(),
            loading_integrations: false,
            installing_integrations: false,
        }));
    }

    pub(super) fn select_settings_section(
        &mut self,
        section: ClientSettingsSection,
        outcome: &mut ClientShellInput,
    ) {
        let request_integrations = matches!(section, ClientSettingsSection::Integrations)
            && matches!(
                self.overlay,
                Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
                    loading_integrations: false,
                    installing_integrations: false,
                    ..
                }))
            );
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.section = section;
            settings.selected = 0;
        }
        if request_integrations {
            self.queue_integration_list(outcome, true);
        }
        outcome.repaint = true;
    }

    fn move_settings_section(&mut self, delta: isize, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_ref() else {
            return;
        };
        let current = ClientSettingsSection::ALL
            .iter()
            .position(|section| *section == settings.section)
            .unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(ClientSettingsSection::ALL.len() as isize)
            as usize;
        self.select_settings_section(ClientSettingsSection::ALL[next], outcome);
    }

    fn settings_choice_count(&self) -> usize {
        match self.overlay.as_ref() {
            Some(ClientShellOverlay::Settings(settings)) => match settings.section {
                ClientSettingsSection::Integrations => settings.integrations.len(),
                section => super::settings_rows::visible_rows(section, &settings.config).count(),
            },
            _ => 0,
        }
    }

    pub(super) fn move_settings_selection(&mut self, delta: isize) {
        let count = self.settings_choice_count();
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() else {
            return;
        };
        if count == 0 {
            settings.selected = 0;
            return;
        }
        settings.selected = (settings.selected as isize + delta)
            .clamp(0, count.saturating_sub(1) as isize) as usize;
    }

    pub(super) fn select_settings_choice(&mut self, index: usize) {
        let count = self.settings_choice_count();
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            if count > 0 {
                settings.selected = index.min(count - 1);
            }
        }
    }

    pub(super) fn cancel_settings_overlay(&mut self) {
        self.overlay = None;
    }

    fn selected_setting_row(&self) -> Option<&'static super::settings_rows::SettingRow> {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_ref() else {
            return None;
        };
        super::settings_rows::visible_rows(settings.section, &settings.config)
            .nth(settings.selected)
    }

    /// Step the selected row's value and save it.
    pub(super) fn step_settings_value(&mut self, delta: isize, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_ref() else {
            return;
        };
        let Some(row) = self.selected_setting_row() else {
            return;
        };
        let Some(value) = row.step(&settings.config, delta) else {
            return;
        };
        let literal = value.literal();
        self.save_settings_edit(
            crate::config::ConfigEdit::Set {
                table: row.table,
                key: row.key,
                value: &literal,
            },
            outcome,
        );
    }

    /// Remove the selected row's key so its default applies.
    fn reset_settings_value(&mut self, outcome: &mut ClientShellInput) {
        let Some(row) = self.selected_setting_row() else {
            return;
        };
        self.save_settings_edit(
            crate::config::ConfigEdit::Reset {
                table: row.table,
                key: row.key,
            },
            outcome,
        );
    }

    fn save_settings_edit(
        &mut self,
        edit: crate::config::ConfigEdit<'_>,
        outcome: &mut ClientShellInput,
    ) {
        outcome.repaint = true;
        if let Err(error) = crate::config::write_edit(edit) {
            self.set_endpoint_error(error);
            return;
        }
        self.reload_client_config();
        self.push_endpoint_method_with_kind(
            crate::api::schema::Method::ServerReloadConfig(
                crate::api::schema::EmptyParams::default(),
            ),
            PendingEndpointKind::ReloadConfig,
            outcome,
        );
        let (config, config_error) = load_settings_config();
        let count = match self.overlay.as_ref() {
            Some(ClientShellOverlay::Settings(settings)) => {
                super::settings_rows::visible_rows(settings.section, &config).count()
            }
            _ => return,
        };
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.config = config;
            settings.config_error = config_error;
            // A change can hide rows below the selection.
            settings.selected = settings.selected.min(count.saturating_sub(1));
        }
    }

    pub(super) fn apply_settings_choice(&mut self, outcome: &mut ClientShellInput) {
        match self.overlay.as_ref() {
            Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
                section: ClientSettingsSection::Integrations,
                ..
            })) => self.install_recommended_integrations(outcome),
            Some(ClientShellOverlay::Settings(_)) => self.step_settings_value(1, outcome),
            _ => {}
        }
    }

    fn queue_integration_list(&mut self, outcome: &mut ClientShellInput, clear_messages: bool) {
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.loading_integrations = true;
            if clear_messages {
                settings.integration_messages.clear();
            }
        }
        if !self.push_endpoint_method_with_kind(
            crate::api::schema::Method::IntegrationList(crate::api::schema::EmptyParams::default()),
            PendingEndpointKind::IntegrationList,
            outcome,
        ) {
            if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                settings.loading_integrations = false;
            }
        }
    }

    fn install_recommended_integrations(&mut self, outcome: &mut ClientShellInput) {
        if self.pending_integration_installs > 0 {
            return;
        }
        let targets = match self.overlay.as_ref() {
            Some(ClientShellOverlay::Settings(settings)) => settings
                .integrations
                .iter()
                .filter(|integration| integration_needs_install(integration))
                .map(|integration| integration.target)
                .collect::<Vec<_>>(),
            _ => return,
        };
        if targets.is_empty() {
            return;
        }
        if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
            settings.installing_integrations = true;
            settings.integration_messages.clear();
        }
        self.pending_integration_installs = 0;
        for target in targets {
            if self.push_endpoint_method_with_kind(
                crate::api::schema::Method::IntegrationInstall(
                    crate::api::schema::IntegrationInstallParams { target },
                ),
                PendingEndpointKind::IntegrationInstall,
                outcome,
            ) {
                self.pending_integration_installs += 1;
            }
        }
        if self.pending_integration_installs == 0 {
            if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                settings.installing_integrations = false;
            }
        }
        outcome.repaint = true;
    }

    pub(super) fn handle_settings_endpoint_result(
        &mut self,
        kind: PendingEndpointKind,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        match kind {
            PendingEndpointKind::IntegrationList => {
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    settings.loading_integrations = false;
                    match result {
                        Ok(crate::api::schema::ResponseResult::IntegrationList {
                            integrations,
                        }) => {
                            settings.integrations = integrations;
                            settings.selected = settings
                                .selected
                                .min(settings.integrations.len().saturating_sub(1));
                        }
                        Ok(_) => {
                            self.set_endpoint_error(
                                "endpoint returned an unexpected integration list result",
                            );
                        }
                        Err(_) => {}
                    }
                }
                (true, Vec::new())
            }
            PendingEndpointKind::IntegrationInstall => {
                let cancelled = result
                    .as_ref()
                    .is_err_and(|error| error.code.as_deref() == Some("endpoint_cancelled"));
                self.pending_integration_installs =
                    self.pending_integration_installs.saturating_sub(1);
                if let Some(ClientShellOverlay::Settings(settings)) = self.overlay.as_mut() {
                    match result {
                        Ok(crate::api::schema::ResponseResult::IntegrationInstall {
                            details,
                            ..
                        }) => settings.integration_messages.extend(details.messages),
                        Ok(_) => settings
                            .integration_messages
                            .push("endpoint returned an unexpected integration result".into()),
                        Err(error) => settings.integration_messages.push(error.message),
                    }
                    settings.installing_integrations = self.pending_integration_installs > 0;
                }
                let actions = if !cancelled
                    && self.pending_integration_installs == 0
                    && matches!(self.overlay, Some(ClientShellOverlay::Settings(_)))
                {
                    let mut deferred = ClientShellInput::default();
                    self.queue_integration_list(&mut deferred, false);
                    deferred.actions
                } else {
                    Vec::new()
                };
                (true, actions)
            }
            _ => (false, Vec::new()),
        }
    }

    pub(super) fn route_settings_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !matches!(self.overlay, Some(ClientShellOverlay::Settings(_))) {
            return false;
        }
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        if code == KeyCode::Esc {
            if !matches!(
                self.overlay,
                Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
                    installing_integrations: true,
                    ..
                }))
            ) {
                self.cancel_settings_overlay();
                outcome.repaint = true;
            }
            return true;
        }
        if code == KeyCode::Tab && modifiers.is_empty() {
            self.move_settings_section(1, outcome);
            return true;
        }
        if code == KeyCode::BackTab && modifiers.difference(KeyModifiers::SHIFT).is_empty() {
            self.move_settings_section(-1, outcome);
            return true;
        }
        if matches!(code, KeyCode::Up | KeyCode::Char('k')) && modifiers.is_empty() {
            self.move_settings_selection(-1);
            outcome.repaint = true;
            return true;
        }
        if matches!(code, KeyCode::Down | KeyCode::Char('j')) && modifiers.is_empty() {
            self.move_settings_selection(1);
            outcome.repaint = true;
            return true;
        }
        if matches!(code, KeyCode::Left | KeyCode::Char('h')) && modifiers.is_empty() {
            self.step_settings_value(-1, outcome);
            return true;
        }
        if matches!(code, KeyCode::Right | KeyCode::Char('l')) && modifiers.is_empty() {
            self.step_settings_value(1, outcome);
            return true;
        }
        if code == KeyCode::Char('r') && modifiers.is_empty() {
            self.reset_settings_value(outcome);
            return true;
        }
        if matches!(code, KeyCode::Enter | KeyCode::Char(' ')) && modifiers.is_empty() {
            self.apply_settings_choice(outcome);
            return true;
        }
        true
    }
}
