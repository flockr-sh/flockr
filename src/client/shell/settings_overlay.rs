use super::*;

pub(super) fn render_settings_overlay(
    buffer: &mut Buffer,
    settings: &ClientSettingsOverlay,
    integration_updates_available: bool,
    palette: &Palette,
) -> Option<OverlayRender> {
    let row_count = match settings.section {
        ClientSettingsSection::Integrations => settings
            .integrations
            .len()
            .max(1)
            .saturating_add(settings.integration_messages.len().min(6))
            .saturating_add(3),
        section => super::super::settings_rows::visible_rows(section, &settings.config).count(),
    };
    // Title, tabs, rule, blank; help, blank; key hints, buttons; borders.
    let height = (row_count as u16).saturating_add(12).max(18);
    let popup = popup(buffer.area, 76, height)?;
    let inner = panel(buffer, popup, palette.accent, palette.panel_bg)?;
    if inner.width < 20 || inner.height < 8 {
        return None;
    }

    put_text(
        buffer,
        inner.x,
        inner.y,
        inner.width,
        " SETTINGS",
        Style::default()
            .fg(palette.text)
            .bg(palette.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    let path = format!("{} ", settings.config_path);
    let path_width = display_width(&path).min(inner.width.saturating_sub(10));
    put_text(
        buffer,
        inner.right().saturating_sub(path_width),
        inner.y,
        path_width,
        &path,
        Style::default().fg(palette.overlay0).bg(palette.panel_bg),
    );

    let integration_badge = integration_updates_available
        || settings
            .integrations
            .iter()
            .any(|integration| integration.state == crate::api::schema::IntegrationState::Outdated);
    let mut tab_x = inner.x;
    let mut tab_hits = Vec::new();
    for section in ClientSettingsSection::ALL {
        let badge = *section == ClientSettingsSection::Integrations && integration_badge;
        let label = if badge {
            format!(" ● {} ", section.label())
        } else {
            format!(" {} ", section.label())
        };
        let width = display_width(&label).min(inner.right().saturating_sub(tab_x));
        let rect = Rect::new(tab_x, inner.y + 1, width, 1);
        let active = *section == settings.section;
        let style = if active {
            Style::default()
                .fg(contrast(palette))
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.overlay1).bg(palette.panel_bg)
        };
        buffer.set_style(rect, style);
        put_text(buffer, rect.x, rect.y, rect.width, &label, style);
        if badge && !active {
            put_text(
                buffer,
                rect.x.saturating_add(1),
                rect.y,
                rect.width.saturating_sub(1).min(2),
                "● ",
                Style::default()
                    .fg(palette.accent)
                    .bg(palette.panel_bg)
                    .add_modifier(Modifier::BOLD),
            );
        }
        tab_hits.push((rect, *section));
        tab_x = tab_x.saturating_add(width.saturating_add(1));
        if tab_x >= inner.right() {
            break;
        }
    }
    put_text(
        buffer,
        inner.x,
        inner.y + 2,
        inner.width,
        &"─".repeat(inner.width as usize),
        Style::default().fg(palette.surface0).bg(palette.panel_bg),
    );

    // Rows, then a blank line and the selected row's help above the hints.
    let content = Rect::new(
        inner.x,
        inner.y + 4,
        inner.width,
        inner.height.saturating_sub(9),
    );
    let mut choice_hits = Vec::new();
    let mut value_hits = Vec::new();
    let help = match settings.section {
        ClientSettingsSection::Integrations => {
            render_integrations(buffer, content, settings, palette);
            None
        }
        section => render_setting_rows(
            buffer,
            content,
            settings,
            section,
            palette,
            &mut choice_hits,
            &mut value_hits,
        ),
    };
    if let Some(help) = settings
        .config_error
        .as_deref()
        .map(|error| format!(" config file has errors, showing defaults: {error}"))
        .or(help)
    {
        put_text(
            buffer,
            inner.x,
            inner.bottom().saturating_sub(4),
            inner.width,
            &help,
            Style::default().fg(palette.overlay1).bg(palette.panel_bg),
        );
    }

    let integrations = settings.section == ClientSettingsSection::Integrations;
    let installable = settings
        .integrations
        .iter()
        .any(super::super::settings::integration_needs_install);
    let show_primary = integrations && installable;
    let labels = if show_primary { vec![12, 12] } else { vec![12] };
    let buttons = row(inner, &labels, 2, inner.height.saturating_sub(1));
    let (primary, close) = if show_primary {
        let primary = buttons[0];
        button(
            buffer,
            primary,
            " ↵ install ",
            Style::default()
                .fg(contrast(palette))
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD),
        );
        (primary, buttons[1])
    } else {
        (Rect::default(), buttons[0])
    };
    button(
        buffer,
        close,
        " esc close ",
        Style::default()
            .fg(palette.text)
            .bg(palette.surface0)
            .add_modifier(Modifier::BOLD),
    );
    put_text(
        buffer,
        inner.x,
        inner.bottom().saturating_sub(2),
        inner.width,
        if integrations {
            " ↑↓ select  tab section"
        } else {
            " ↑↓ select  ←→ change  r reset to default  tab section"
        },
        Style::default().fg(palette.overlay1).bg(palette.panel_bg),
    );

    Some(OverlayRender {
        area: popup,
        primary,
        cancel: close,
        settings_popup: popup,
        settings_tabs: tab_hits,
        settings_choices: choice_hits,
        settings_values: value_hits,
        ..OverlayRender::default()
    })
}

/// Draw one section's rows as `label  ‹ value ›` and return the selected
/// row's help text. Values that differ from the default use the accent color.
fn render_setting_rows(
    buffer: &mut Buffer,
    area: Rect,
    settings: &ClientSettingsOverlay,
    section: ClientSettingsSection,
    palette: &Palette,
    row_hits: &mut Vec<(Rect, usize)>,
    value_hits: &mut Vec<(Rect, usize)>,
) -> Option<String> {
    const LABEL_WIDTH: u16 = 27;
    let defaults = crate::config::Config::default();
    let rows =
        super::super::settings_rows::visible_rows(section, &settings.config).collect::<Vec<_>>();
    let visible = usize::from(area.height);
    let scroll = settings.selected.saturating_sub(visible.saturating_sub(1));
    for (index, setting) in rows.iter().enumerate().skip(scroll).take(visible) {
        let y = area.y + (index - scroll) as u16;
        let rect = Rect::new(area.x, y, area.width, 1);
        let selected = index == settings.selected;
        let base = if selected {
            Style::default().fg(palette.text).bg(palette.surface0)
        } else {
            Style::default().fg(palette.subtext0).bg(palette.panel_bg)
        };
        buffer.set_style(rect, base);
        let marker = if selected { "▸" } else { " " };
        put_text(
            buffer,
            rect.x,
            y,
            LABEL_WIDTH.min(rect.width),
            &format!(" {marker} {}", setting.label),
            if selected {
                base.add_modifier(Modifier::BOLD)
            } else {
                base
            },
        );
        let changed = setting.current_index(&settings.config) != setting.current_index(&defaults);
        let value = format!("‹ {} ›", setting.current_label(&settings.config));
        let value_x = rect.x.saturating_add(LABEL_WIDTH).min(rect.right());
        let value_width = display_width(&value).min(rect.right().saturating_sub(value_x));
        put_text(
            buffer,
            value_x,
            y,
            value_width,
            &value,
            if changed {
                base.fg(palette.accent).add_modifier(Modifier::BOLD)
            } else {
                base
            },
        );
        row_hits.push((rect, index));
        value_hits.push((Rect::new(value_x, y, value_width, 1), index));
    }
    rows.get(settings.selected)
        .map(|setting| format!(" {}", setting.help))
}

fn render_integrations(
    buffer: &mut Buffer,
    area: Rect,
    settings: &ClientSettingsOverlay,
    palette: &Palette,
) {
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        "agent integrations",
        Style::default()
            .fg(palette.text)
            .bg(palette.panel_bg)
            .add_modifier(Modifier::BOLD),
    );
    put_text(
        buffer,
        area.x,
        area.y + 1,
        area.width,
        "enable session restore and, where supported, direct status updates",
        Style::default().fg(palette.overlay1).bg(palette.panel_bg),
    );
    if settings.loading_integrations {
        put_text(
            buffer,
            area.x,
            area.y + 3,
            area.width,
            " loading integrations…",
            Style::default().fg(palette.overlay1).bg(palette.panel_bg),
        );
        return;
    }
    if settings.integrations.is_empty() {
        put_text(
            buffer,
            area.x,
            area.y + 3,
            area.width,
            " no integration targets available",
            Style::default().fg(palette.overlay1).bg(palette.panel_bg),
        );
        return;
    }
    for (index, integration) in settings.integrations.iter().enumerate() {
        let y = area.y + 3 + index as u16;
        if y >= area.bottom() {
            break;
        }
        let (marker, color, status) = match integration.state {
            crate::api::schema::IntegrationState::Current => ("✓", palette.green, "installed"),
            crate::api::schema::IntegrationState::Outdated => {
                ("↻", palette.yellow, "update available")
            }
            crate::api::schema::IntegrationState::NotInstalled if integration.available => {
                ("+", palette.accent, "available")
            }
            crate::api::schema::IntegrationState::NotInstalled => {
                ("–", palette.overlay0, "not found")
            }
        };
        put_text(
            buffer,
            area.x,
            y,
            3,
            &format!(" {marker}"),
            Style::default().fg(color).bg(palette.panel_bg),
        );
        put_text(
            buffer,
            area.x + 3,
            y,
            11.min(area.width.saturating_sub(3)),
            &format!("{:<9}", integration.label),
            Style::default().fg(palette.subtext0).bg(palette.panel_bg),
        );
        put_text(
            buffer,
            area.x + 14,
            y,
            area.width.saturating_sub(14),
            status,
            Style::default().fg(palette.overlay1).bg(palette.panel_bg),
        );
    }
    let message_y = area
        .y
        .saturating_add(4)
        .saturating_add(settings.integrations.len() as u16);
    for (offset, message) in settings.integration_messages.iter().take(6).enumerate() {
        let y = message_y.saturating_add(offset as u16);
        if y >= area.bottom() {
            break;
        }
        put_text(
            buffer,
            area.x,
            y,
            area.width,
            &format!(" {message}"),
            Style::default().fg(palette.overlay1).bg(palette.panel_bg),
        );
    }
    if settings.installing_integrations && message_y < area.bottom() {
        put_text(
            buffer,
            area.x,
            message_y,
            area.width,
            " installing…",
            Style::default().fg(palette.overlay1).bg(palette.panel_bg),
        );
    }
}
