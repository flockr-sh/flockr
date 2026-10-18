use super::*;
use ratatui::{
    text::Line,
    widgets::{Paragraph, Widget},
};

fn workspace_selection_background(palette: &Palette) -> ratatui::style::Color {
    if palette.selection_bg == ratatui::style::Color::Reset {
        palette.active_row_bg
    } else {
        palette.selection_bg
    }
}

pub(in crate::client::shell) fn workspace_active_background(
    palette: &Palette,
    navigating: bool,
) -> ratatui::style::Color {
    // The fallback cursor shares the active-row color; only fill the cursor while navigating.
    if navigating && palette.selection_bg == ratatui::style::Color::Reset {
        palette.sidebar_bg
    } else {
        palette.active_row_bg
    }
}

/// Background for header bands and chips. Themes like `terminal` leave
/// surface0 unset, so fall back to the next surface.
fn chrome_fill(palette: &Palette) -> ratatui::style::Color {
    if matches!(palette.surface0, ratatui::style::Color::Reset)
        || palette.surface0 == palette.sidebar_bg
    {
        palette.surface1
    } else {
        palette.surface0
    }
}

/// Draw a section header row: the title, an optional count, and the
/// configured band.
pub(in crate::client::shell) fn render_section_header(
    buffer: &mut Buffer,
    row: Rect,
    title: &str,
    count: Option<usize>,
    config: &ClientShellConfig,
) {
    use crate::config::SidebarHeaderStyle;

    if row.is_empty() {
        return;
    }
    let palette = &config.palette;
    let chrome = config.sidebar_chrome;
    let row = Rect::new(row.x, row.y, row.width, 1);
    let (title_style, count_style) = match chrome.header {
        SidebarHeaderStyle::Band => {
            buffer.set_style(row, Style::default().bg(chrome_fill(palette)));
            (
                Style::default()
                    .fg(palette.text)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(palette.subtext0),
            )
        }
        SidebarHeaderStyle::Plain => (
            Style::default()
                .fg(palette.overlay0)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(palette.overlay0),
        ),
    };
    let x = put_segment(buffer, row.x, row.y, row.right(), " ", title_style);
    let x = put_segment(buffer, x, row.y, row.right(), title, title_style);
    if let Some(count) = count.filter(|_| chrome.header_counts) {
        put_segment(
            buffer,
            x,
            row.y,
            row.right(),
            &format!(" {count}"),
            count_style,
        );
    }
}

/// A sidebar button's label in chip style and in plain text style.
#[derive(Clone, Copy)]
pub(in crate::client::shell) struct SidebarButtonLabel<'a> {
    pub(in crate::client::shell) chip: &'a str,
    pub(in crate::client::shell) text: &'a str,
}

/// Draw a clickable label at `x` and return the rect it covers. Chips get a
/// space of padding on each side; `attention` draws the accent version.
pub(in crate::client::shell) fn render_sidebar_button(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    max_right: u16,
    label: SidebarButtonLabel<'_>,
    attention: bool,
    on_band: bool,
    config: &ClientShellConfig,
) -> Rect {
    let palette = &config.palette;
    let (text, style) = match config.sidebar_chrome.buttons {
        crate::config::SidebarButtonStyle::Text => (
            label.text.to_owned(),
            Style::default()
                .fg(if attention {
                    palette.accent
                } else {
                    palette.overlay0
                })
                .add_modifier(if on_band {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        crate::config::SidebarButtonStyle::Chip => {
            let style = if attention {
                Style::default()
                    .fg(chrome_fill(palette))
                    .bg(palette.accent)
                    .add_modifier(Modifier::BOLD)
            } else if on_band
                && config.sidebar_chrome.header == crate::config::SidebarHeaderStyle::Band
            {
                // A chip in the band color would vanish, so flip it.
                Style::default()
                    .fg(chrome_fill(palette))
                    .bg(palette.subtext0)
            } else {
                Style::default().fg(palette.text).bg(chrome_fill(palette))
            };
            (format!(" {} ", label.chip), style)
        }
    };
    let width = display_width(&text).min(max_right.saturating_sub(x));
    let rect = Rect::new(x, y, width, 1);
    put_text(buffer, x, y, width, &text, style);
    rect
}

/// Width `render_sidebar_button` will use for `label`.
pub(in crate::client::shell) fn sidebar_button_width(
    label: SidebarButtonLabel<'_>,
    config: &ClientShellConfig,
) -> u16 {
    match config.sidebar_chrome.buttons {
        crate::config::SidebarButtonStyle::Chip => display_width(label.chip) + 2,
        crate::config::SidebarButtonStyle::Text => display_width(label.text),
    }
}

/// Draw the sidebar's title row: the collapse toggle, name, and version on
/// the left, the menu button on the right. Returns the area below it.
pub(in crate::client::shell) fn render_sidebar_title(
    buffer: &mut Buffer,
    area: Rect,
    menu_attention: bool,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) -> Rect {
    if area.is_empty() {
        return area;
    }
    let palette = &config.palette;
    let row = Rect::new(area.x, area.y, area.width, 1);
    // The toggle keeps its cell even when the sidebar is too narrow for the rest.
    hits.sidebar_toggle = Rect::new(row.x, row.y, 1, 1);
    put_text(
        buffer,
        row.x,
        row.y,
        1,
        collapse_arrow(config, true),
        Style::default().fg(palette.overlay0),
    );
    let left = row.x.saturating_add(2).min(row.right());
    let mut right = row.right();
    if config.mouse_capture && left < row.right() {
        let label = SidebarButtonLabel {
            chip: if menu_attention {
                "● menu"
            } else {
                "≡ menu"
            },
            text: if menu_attention { "● menu" } else { "menu" },
        };
        hits.global_launcher = render_sidebar_button(
            buffer,
            row.right()
                .saturating_sub(sidebar_button_width(label, config))
                .max(left),
            row.y,
            row.right(),
            label,
            menu_attention,
            false,
            config,
        );
        right = hits.global_launcher.x.saturating_sub(1).max(left);
    }
    let x = put_segment(
        buffer,
        left,
        row.y,
        right,
        "flockr",
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
    );
    let version = concat!(" v", env!("CARGO_PKG_VERSION"));
    if x.saturating_add(display_width(version)) <= right {
        put_segment(
            buffer,
            x,
            row.y,
            right,
            version,
            Style::default().fg(palette.overlay0),
        );
    }
    Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    )
}

/// Draw the new-space button at the right of a section header row.
pub(in crate::client::shell) fn render_new_workspace_button(
    buffer: &mut Buffer,
    header: Rect,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    if !config.mouse_capture || header.is_empty() {
        return;
    }
    let label = SidebarButtonLabel {
        chip: "+",
        text: "+",
    };
    hits.new_workspace = render_sidebar_button(
        buffer,
        header
            .right()
            .saturating_sub(sidebar_button_width(label, config))
            .max(header.x),
        header.y,
        header.right(),
        label,
        false,
        true,
        config,
    );
}

/// Draw the line between the sidebar's two sections.
pub(in crate::client::shell) fn render_section_divider(
    buffer: &mut Buffer,
    row: Rect,
    grip: bool,
    config: &ClientShellConfig,
) {
    use crate::config::SidebarDividerStyle;

    let palette = &config.palette;
    let divider = config.sidebar_chrome.divider;
    let color = match divider {
        SidebarDividerStyle::Dim => palette.surface_dim,
        SidebarDividerStyle::Line | SidebarDividerStyle::Handle => palette.surface1,
    };
    put_text(
        buffer,
        row.x,
        row.y,
        row.width,
        &"─".repeat(row.width as usize),
        Style::default().fg(color),
    );
    if grip && divider == SidebarDividerStyle::Handle && row.width >= 7 {
        put_text(
            buffer,
            row.x + (row.width - 3) / 2,
            row.y,
            3,
            "━━━",
            Style::default().fg(palette.overlay0),
        );
    }
}

/// Mark a space grabbed in navigate mode, with the marker at `marker_x`.
pub(in crate::client::shell) fn render_grab_marker(
    buffer: &mut Buffer,
    rect: Rect,
    marker_x: u16,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    match config.sidebar_chrome.grab {
        crate::config::SidebarGrabStyle::Fill => {
            buffer.set_style(rect, Style::default().bg(palette.accent));
        }
        crate::config::SidebarGrabStyle::Marker => {
            if marker_x < rect.right() {
                put_text(
                    buffer,
                    marker_x,
                    rect.y,
                    1,
                    "↕",
                    Style::default()
                        .fg(palette.peach)
                        .add_modifier(Modifier::BOLD),
                );
            }
        }
    }
}

/// Indent for rows after the first: under the first text token after the
/// icon and number, or the usual 3 columns when the first row starts with text.
pub(in crate::client::shell) fn continuation_indent(
    rows: &[Vec<crate::ui::ResolvedToken>],
    state_icon: &str,
) -> u16 {
    match rows
        .first()
        .map(|row| crate::ui::leading_gutter_width(row, state_icon))
    {
        Some(width) if width > 0 => 1 + width.min(u16::MAX as usize - 1) as u16,
        _ => 3,
    }
}

/// Marks the edges of one sidebar group (a space with its worktrees, or the
/// agents of one space) inside `rect`.
pub(in crate::client::shell) fn render_group_border(
    buffer: &mut Buffer,
    rect: Rect,
    border: crate::config::SidebarBorder,
    group_start: bool,
    group_end: bool,
    palette: &Palette,
) {
    if rect.is_empty() {
        return;
    }
    match border {
        crate::config::SidebarBorder::None => {}
        crate::config::SidebarBorder::Underline => {
            if group_end {
                buffer.set_style(
                    Rect::new(rect.x, rect.bottom() - 1, rect.width, 1),
                    Style::default()
                        .add_modifier(Modifier::UNDERLINED)
                        .underline_color(palette.surface1),
                );
            }
        }
        crate::config::SidebarBorder::Bracket => {
            let last = rect.bottom() - 1;
            for y in rect.top()..rect.bottom() {
                let symbol = match (y == rect.top() && group_start, y == last && group_end) {
                    (true, true) => "╶",
                    (true, false) => "╭",
                    (false, true) => "╰",
                    (false, false) => "│",
                };
                buffer[(rect.x, y)]
                    .set_symbol(symbol)
                    .set_fg(palette.overlay0);
            }
        }
    }
}

fn number_label(key: Option<char>) -> String {
    key.map_or_else(|| "  ".to_owned(), |key| format!("{key:<2}"))
}

pub(in crate::client::shell) fn collapsed_sidebar_sections(
    content: Rect,
) -> (Rect, Option<u16>, Rect) {
    if content.is_empty() {
        return (Rect::default(), None, Rect::default());
    }
    if content.height < 7 {
        return (content, None, Rect::default());
    }
    let workspace_height = content.height.div_ceil(2);
    let divider_y = content.y + workspace_height;
    let detail_height = content.height.saturating_sub(workspace_height + 1);
    (
        Rect::new(content.x, content.y, content.width, workspace_height),
        Some(divider_y),
        Rect::new(content.x, divider_y + 1, content.width, detail_height),
    )
}

pub(crate) fn render_collapsed_sidebar(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    selected_workspace_id: Option<&str>,
    dragged_workspace_id: Option<&str>,
    collapsed_groups: &HashSet<String>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    let selection_background = workspace_selection_background(palette);
    let active_background = workspace_active_background(palette, selected_workspace_id.is_some());
    let content = render_sidebar_background(buffer, area, config);
    let (workspace_area, divider_y, detail_area) = collapsed_sidebar_sections(content);
    // Numbers follow navigation order, which the switch keys use.
    let mut navigation_position = vec![None; snapshot.workspaces.len()];
    for (position, entry) in workspace_entries(snapshot, collapsed_groups)
        .iter()
        .enumerate()
    {
        if let Some(slot) = navigation_position.get_mut(entry.index) {
            *slot = Some(position);
        }
    }
    let switch_workspace = &config.keybinds.keybinds.switch_workspace;
    for (index, workspace) in snapshot
        .workspaces
        .iter()
        .take(workspace_area.height as usize)
        .enumerate()
    {
        let rect = Rect::new(
            workspace_area.x,
            workspace_area.y + index as u16,
            workspace_area.width,
            1,
        );
        let selected = selected_workspace_id == Some(workspace.workspace_id.as_str());
        if selected {
            buffer.set_style(rect, Style::default().bg(selection_background));
        } else if workspace.focused {
            buffer.set_style(rect, Style::default().bg(active_background));
        }
        let number_style = if selected {
            Style::default()
                .fg(palette.overlay1)
                .bg(selection_background)
        } else if workspace.focused {
            Style::default().fg(palette.text).bg(active_background)
        } else {
            Style::default().fg(palette.overlay0)
        };
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width.min(2),
            &number_label(
                navigation_position[index]
                    .and_then(|position| crate::config::indexed_key(switch_workspace, position)),
            ),
            number_style,
        );
        let status = workspace.agent_status;
        put_text(
            buffer,
            rect.x.saturating_add(2),
            rect.y,
            rect.width.saturating_sub(2),
            status_icon(status, config.status_indicators),
            Style::default().fg(status_icon_color(status, config)),
        );
        if selected && dragged_workspace_id == Some(workspace.workspace_id.as_str()) {
            // Grabbed in navigate mode: the marker replaces the status icon.
            render_grab_marker(buffer, rect, rect.x.saturating_add(2), config);
        }
        hits.workspaces.push(WorkspaceHit {
            rect,
            endpoint_id: ClientEndpointId::Local,
            workspace_id: workspace.workspace_id.clone(),
            indented: false,
            group_toggle: None,
        });
    }

    if let Some(divider_y) = divider_y {
        render_section_divider(
            buffer,
            Rect::new(workspace_area.x, divider_y, workspace_area.width, 1),
            false,
            config,
        );
    }

    let detail_content = Rect::new(
        detail_area.x,
        detail_area.y,
        detail_area.width,
        detail_area.height.saturating_sub(1),
    );
    for (index, pane_id) in super::ordered_agent_pane_ids(snapshot, config.agent_panel_filter)
        .into_iter()
        .take(detail_content.height as usize)
        .enumerate()
    {
        let Some(agent) = snapshot
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane_id)
        else {
            continue;
        };
        let rect = Rect::new(
            detail_content.x,
            detail_content.y + index as u16,
            detail_content.width,
            1,
        );
        if agent.focused {
            buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
        }
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width.min(2),
            &number_label(crate::config::indexed_key(
                &config.keybinds.keybinds.focus_agent,
                index,
            )),
            Style::default().fg(if agent.focused {
                palette.text
            } else {
                palette.overlay0
            }),
        );
        put_text(
            buffer,
            rect.x.saturating_add(2),
            rect.y,
            rect.width.saturating_sub(2),
            status_icon(agent.agent_status, config.status_indicators),
            Style::default().fg(status_icon_color(agent.agent_status, config)),
        );
        hits.agents.push((rect, pane_id));
    }
    hits.sidebar_toggle = render_collapsed_toggle(
        buffer,
        area,
        workspace_area,
        super::super::global_menu::global_menu_attention(snapshot),
        config,
    );
}

/// Arrow for the collapse or expand toggle, pointing the way the sidebar moves.
fn collapse_arrow(config: &ClientShellConfig, collapse: bool) -> &'static str {
    let toward_left = collapse == (config.sidebar_position == SidebarPositionConfig::Left);
    if toward_left {
        "«"
    } else {
        "»"
    }
}

/// Draw the collapsed sidebar's expand button, centered on the bottom row.
pub(in crate::client::shell) fn render_collapsed_toggle(
    buffer: &mut Buffer,
    area: Rect,
    workspace_area: Rect,
    attention: bool,
    config: &ClientShellConfig,
) -> Rect {
    if area.is_empty() || workspace_area.width == 0 {
        return Rect::default();
    }
    let label = SidebarButtonLabel {
        chip: collapse_arrow(config, false),
        text: collapse_arrow(config, false),
    };
    let width = sidebar_button_width(label, config).min(workspace_area.width);
    let x = workspace_area.x + (workspace_area.width - width).div_ceil(2);
    let y = area.bottom().saturating_sub(1);
    if config.sidebar_chrome.buttons == crate::config::SidebarButtonStyle::Text {
        let palette = &config.palette;
        put_text(
            buffer,
            x,
            y,
            1,
            collapse_arrow(config, false),
            if attention {
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette.overlay0)
            },
        );
        return Rect::new(x, y, 1, 1);
    }
    render_sidebar_button(
        buffer,
        x,
        y,
        workspace_area.right(),
        label,
        attention,
        false,
        config,
    )
}

pub(crate) fn render_sidebar(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    state: &mut ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    let content = render_sidebar_background(buffer, area, config);
    hits.sidebar_divider = sidebar_separator(area, config.sidebar_position);
    let attention = super::super::global_menu::global_menu_attention(snapshot);
    let sections = render_sidebar_title(buffer, content, attention, config, hits);
    hits.sidebar_sections = sections;
    let (workspace_area, detail_area) =
        crate::ui::expanded_sidebar_sections(sections, state.sidebar_section_split);
    hits.sidebar_section_divider =
        crate::ui::sidebar_section_divider_rect(sections, state.sidebar_section_split);
    render_section_header(
        buffer,
        workspace_area,
        "SPACES",
        Some(snapshot.workspaces.len()),
        config,
    );
    render_new_workspace_button(buffer, workspace_area, config, hits);

    let entries = workspace_entries(snapshot, state.collapsed_groups);
    let switch_workspace = &config.keybinds.keybinds.switch_workspace;
    let body = Rect::new(
        workspace_area.x,
        workspace_area.y.saturating_add(WORKSPACE_HEADER_ROWS),
        workspace_area.width,
        workspace_area.height.saturating_sub(WORKSPACE_HEADER_ROWS),
    );
    hits.workspace_body = body;
    let row_heights = entries
        .iter()
        .enumerate()
        .map(|(position, entry)| {
            snapshot
                .workspaces
                .get(entry.index)
                .map(|workspace| {
                    workspace_rows(
                        workspace,
                        displayed_workspace_status(snapshot, workspace, state.collapsed_groups),
                        entry.indented,
                        crate::config::indexed_key(switch_workspace, position),
                        &config.spaces,
                    )
                    .len()
                    .max(1)
                    .min(u16::MAX as usize) as u16
                })
                .unwrap_or(1)
        })
        .collect::<Vec<_>>();
    let gaps = entries
        .iter()
        .enumerate()
        .map(|(index, _)| {
            entries
                .get(index + 1)
                .map_or(0, |next| u16::from(!next.indented) * config.spaces.row_gap)
        })
        .collect::<Vec<_>>();
    let mut metrics = super::scroll::list_scroll_metrics(
        &row_heights,
        &gaps,
        body.height,
        *state.workspace_scroll,
    );
    if !body.is_empty() && std::mem::take(state.reveal_focused_workspace) {
        if let Some(target) = entries
            .iter()
            .position(|entry| snapshot.workspaces[entry.index].focused)
        {
            *state.workspace_scroll = super::scroll::list_scroll_start_to_reveal(
                &row_heights,
                &gaps,
                body.height,
                *state.workspace_scroll,
                target,
            );
            metrics = super::scroll::list_scroll_metrics(
                &row_heights,
                &gaps,
                body.height,
                *state.workspace_scroll,
            );
        }
    }
    hits.workspace_max_scroll = metrics.max_offset_from_bottom;
    hits.workspace_scroll_metrics = Some(metrics);
    *state.workspace_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for (entry_position, entry) in entries.iter().enumerate().skip(*state.workspace_scroll) {
        let Some(workspace) = snapshot.workspaces.get(entry.index) else {
            continue;
        };
        let status = displayed_workspace_status(snapshot, workspace, state.collapsed_groups);
        let number = crate::config::indexed_key(switch_workspace, entry_position);
        let rows = workspace_rows(workspace, status, entry.indented, number, &config.spaces);
        let row_height = (rows.len().max(1).min(u16::MAX as usize) as u16).min(body.height);
        if y.saturating_add(row_height) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, content_width, row_height);
        let selected = state.selected_workspace_id.is_some_and(|target| {
            target.matches(state.active_endpoint_id, &workspace.workspace_id)
        });
        let dragged = state.dragged_workspace_id == Some(workspace.workspace_id.as_str());
        if selected {
            buffer.set_style(rect, Style::default().bg(palette.selection_bg));
        } else if dragged {
            buffer.set_style(rect, Style::default().bg(palette.surface1));
        } else if workspace.focused {
            buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
        }
        let content = if config.spaces.border == crate::config::SidebarBorder::Bracket {
            Rect::new(
                rect.x + 1,
                rect.y,
                rect.width.saturating_sub(1),
                rect.height,
            )
        } else {
            rect
        };
        render_workspace_rows(
            buffer,
            content,
            status,
            config,
            entry,
            rows,
            workspace.focused,
            selected,
            state.selected_workspace_id.is_some(),
            dragged,
            palette,
        );
        if selected && dragged {
            // Grabbed in navigate mode: the workspace keys move it.
            // Left of the group toggle column.
            if rect.width >= 4 {
                render_grab_marker(buffer, rect, rect.right().saturating_sub(2), config);
            }
        }
        render_group_border(
            buffer,
            rect,
            config.spaces.border,
            !entry.indented,
            entries
                .get(entry_position + 1)
                .is_none_or(|next| !next.indented),
            palette,
        );
        let group_toggle = render_parent_group_toggle(
            buffer,
            rect,
            snapshot,
            entry.index,
            state.collapsed_groups,
            palette,
        );
        hits.workspaces.push(WorkspaceHit {
            rect,
            endpoint_id: ClientEndpointId::Local,
            workspace_id: workspace.workspace_id.clone(),
            indented: entry.indented,
            group_toggle,
        });
        let gap = entries
            .get(entry_position + 1)
            .map_or(0, |next| u16::from(!next.indented) * config.spaces.row_gap);
        y = y.saturating_add(row_height + gap);
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.workspace_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, palette);
    }

    if let Some(row) = state
        .workspace_drop_indicator_row
        .filter(|row| *row >= workspace_area.y.saturating_add(1) && *row < workspace_area.bottom())
    {
        put_text(
            buffer,
            body.x,
            row,
            body.width,
            &"─".repeat(body.width as usize),
            Style::default().fg(palette.accent),
        );
    }

    super::render_agent_panel(
        buffer,
        detail_area,
        snapshot,
        config,
        state.agent_scroll,
        hits,
    );
}

pub(crate) fn workspace_entries(
    snapshot: &ClientShellSnapshot,
    collapsed_groups: &HashSet<String>,
) -> Vec<WorkspaceEntry> {
    let mut members = HashMap::<&str, Vec<usize>>::new();
    for (index, workspace) in snapshot.workspaces.iter().enumerate() {
        if let Some(worktree) = &workspace.worktree {
            members.entry(&worktree.key).or_default().push(index);
        }
    }
    let grouped = members
        .iter()
        .filter(|(_, indices)| {
            indices.iter().any(|index| {
                snapshot.workspaces[*index]
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| worktree.is_linked_worktree)
            }) && indices.iter().any(|index| {
                snapshot.workspaces[*index]
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| !worktree.is_linked_worktree)
            })
        })
        .map(|(key, _)| *key)
        .collect::<HashSet<_>>();
    let mut emitted = HashSet::<&str>::new();
    let mut entries = Vec::new();
    for (index, workspace) in snapshot.workspaces.iter().enumerate() {
        let Some(worktree) = workspace
            .worktree
            .as_ref()
            .filter(|worktree| grouped.contains(worktree.key.as_str()))
        else {
            entries.push(WorkspaceEntry {
                index,
                indented: false,
                last_child: false,
            });
            continue;
        };
        if !emitted.insert(&worktree.key) {
            continue;
        }
        let Some(group_members) = members.get(worktree.key.as_str()) else {
            continue;
        };
        for parent in group_members.iter().copied().filter(|member| {
            snapshot.workspaces[*member]
                .worktree
                .as_ref()
                .is_some_and(|worktree| !worktree.is_linked_worktree)
        }) {
            entries.push(WorkspaceEntry {
                index: parent,
                indented: false,
                last_child: false,
            });
        }
        if collapsed_groups.contains(&worktree.key) {
            if let Some(active) = group_members.iter().copied().find(|member| {
                let workspace = &snapshot.workspaces[*member];
                workspace.focused
                    && workspace
                        .worktree
                        .as_ref()
                        .is_some_and(|worktree| worktree.is_linked_worktree)
            }) {
                entries.push(WorkspaceEntry {
                    index: active,
                    indented: true,
                    last_child: true,
                });
            }
            continue;
        }
        let children = group_members
            .iter()
            .copied()
            .filter(|member| {
                snapshot.workspaces[*member]
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| worktree.is_linked_worktree)
            })
            .collect::<Vec<_>>();
        for (child_index, child) in children.iter().enumerate() {
            entries.push(WorkspaceEntry {
                index: *child,
                indented: true,
                last_child: child_index + 1 == children.len(),
            });
        }
    }
    entries
}

/// Name of a worktree space drawn under its repo's main checkout: its branch,
/// unless the user renamed it.
pub(in crate::client::shell) fn nested_worktree_label(workspace: &ClientShellWorkspace) -> &str {
    if workspace.custom_label {
        return &workspace.label;
    }
    workspace
        .branch
        .as_deref()
        .map(short_worktree_branch)
        .unwrap_or(&workspace.label)
}

/// Drops the `worktree/` namespace that new worktree branches get.
pub(in crate::client::shell) fn short_worktree_branch(branch: &str) -> &str {
    branch.strip_prefix("worktree/").unwrap_or(branch)
}

/// The main checkout a worktree space is drawn under in the spaces list, if
/// that checkout is open as a space.
pub(in crate::client::shell) fn worktree_parent<'a>(
    snapshot: &'a ClientShellSnapshot,
    workspace: &ClientShellWorkspace,
) -> Option<&'a ClientShellWorkspace> {
    let worktree = workspace
        .worktree
        .as_ref()
        .filter(|worktree| worktree.is_linked_worktree)?;
    snapshot.workspaces.iter().find(|candidate| {
        candidate
            .worktree
            .as_ref()
            .is_some_and(|candidate| candidate.key == worktree.key && !candidate.is_linked_worktree)
    })
}

fn parent_group_key(snapshot: &ClientShellSnapshot, index: usize) -> Option<String> {
    let workspace = snapshot.workspaces.get(index)?;
    let worktree = workspace.worktree.as_ref()?;
    if worktree.is_linked_worktree {
        return None;
    }
    snapshot
        .workspaces
        .iter()
        .any(|candidate| {
            candidate.worktree.as_ref().is_some_and(|candidate| {
                candidate.key == worktree.key && candidate.is_linked_worktree
            })
        })
        .then(|| worktree.key.clone())
}

pub(in crate::client::shell) fn workspace_close_is_group(
    snapshot: &ClientShellSnapshot,
    workspace: &ClientShellWorkspace,
) -> bool {
    let Some(worktree) = workspace
        .worktree
        .as_ref()
        .filter(|worktree| !worktree.is_linked_worktree)
    else {
        return false;
    };
    let mut has_child = false;
    for member in &snapshot.workspaces {
        if member.workspace_id == workspace.workspace_id {
            continue;
        }
        if let Some(candidate) = member
            .worktree
            .as_ref()
            .filter(|candidate| candidate.key == worktree.key)
        {
            if !candidate.is_linked_worktree {
                return false;
            }
            has_child = true;
        }
    }
    has_child
}

pub(in crate::client::shell) fn render_parent_group_toggle(
    buffer: &mut Buffer,
    workspace_rect: Rect,
    snapshot: &ClientShellSnapshot,
    workspace_index: usize,
    collapsed_groups: &HashSet<String>,
    palette: &Palette,
) -> Option<(Rect, String)> {
    let key = parent_group_key(snapshot, workspace_index)?;
    let toggle = Rect::new(
        workspace_rect.right().saturating_sub(1),
        workspace_rect.y,
        1,
        1,
    );
    put_text(
        buffer,
        toggle.x,
        toggle.y,
        toggle.width,
        if collapsed_groups.contains(&key) {
            "▸"
        } else {
            "▾"
        },
        Style::default().fg(palette.accent),
    );
    Some((toggle, key))
}

pub(in crate::client::shell) fn displayed_workspace_status(
    snapshot: &ClientShellSnapshot,
    workspace: &ClientShellWorkspace,
    collapsed_groups: &HashSet<String>,
) -> crate::api::schema::AgentStatus {
    let Some(worktree) = workspace
        .worktree
        .as_ref()
        .filter(|worktree| !worktree.is_linked_worktree)
    else {
        return workspace.agent_status;
    };
    if !collapsed_groups.contains(&worktree.key) {
        return workspace.agent_status;
    }
    snapshot
        .workspaces
        .iter()
        .filter(|candidate| {
            candidate.worktree.as_ref().is_some_and(|member| {
                member.key == worktree.key
                    && (member.is_linked_worktree
                        || candidate.workspace_id == workspace.workspace_id)
            })
        })
        .map(|candidate| candidate.agent_status)
        .max_by_key(|status| status_priority(*status))
        .unwrap_or(workspace.agent_status)
}

pub(in crate::client::shell) fn workspace_rows(
    workspace: &ClientShellWorkspace,
    status: crate::api::schema::AgentStatus,
    indented: bool,
    number: Option<char>,
    config: &SpacesSidebarConfig,
) -> Vec<Vec<crate::ui::ResolvedToken>> {
    let label = if indented {
        nested_worktree_label(workspace)
    } else {
        &workspace.label
    };
    let token_values = workspace.tokens.iter().cloned().collect::<HashMap<_, _>>();
    crate::ui::sidebar_space_rows(
        config,
        crate::ui::SpaceTokenContext {
            workspace: label,
            branch: workspace.branch.as_deref(),
            state_text: status_text(status),
            ahead_behind: workspace.git_ahead_behind,
            diff_stat: workspace.git_diff_stat,
            number,
            tokens: &token_values,
            suppress_branch: indented,
        },
    )
}

pub(in crate::client::shell) fn render_workspace_rows(
    buffer: &mut Buffer,
    area: Rect,
    status: crate::api::schema::AgentStatus,
    config: &ClientShellConfig,
    entry: &WorkspaceEntry,
    rows: Vec<Vec<crate::ui::ResolvedToken>>,
    focused: bool,
    selected: bool,
    navigating: bool,
    dragged: bool,
    palette: &Palette,
) {
    for (row_index, row) in rows.iter().enumerate() {
        let y = area.y + row_index as u16;
        if y >= area.bottom() {
            break;
        }
        let mut x = area.x;
        if entry.indented {
            let prefix = if row_index == 0 {
                if entry.last_child {
                    "   └─ "
                } else {
                    "   ├─ "
                }
            } else if entry.last_child {
                "        "
            } else {
                "   │    "
            };
            x = put_segment(
                buffer,
                x,
                y,
                area.right(),
                prefix,
                Style::default().fg(palette.overlay0),
            );
        } else if row_index == 0 {
            x = x.saturating_add(1);
        } else {
            x = x.saturating_add(continuation_indent(
                &rows,
                status_icon(status, config.status_indicators),
            ));
        }
        let highlighted = focused || dragged;
        let workspace_style = Style::default()
            .fg(if highlighted {
                palette.text
            } else {
                palette.subtext0
            })
            .add_modifier(if highlighted {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        let secondary_style = Style::default().fg(if focused {
            palette.mauve
        } else {
            palette.overlay0
        });
        let spans = crate::ui::resolved_token_spans(
            row,
            (
                status_icon(status, config.status_indicators),
                Style::default().fg(status_icon_color(status, config)),
            ),
            Style::default().fg(status_color(status, palette)),
            workspace_style,
            secondary_style,
            Style::default().fg(palette.overlay1),
            palette,
            area.right().saturating_sub(2).saturating_sub(x) as usize,
        );
        Paragraph::new(Line::from(spans)).render(
            Rect::new(x, y, area.right().saturating_sub(2).saturating_sub(x), 1),
            buffer,
        );
    }

    let background = if selected {
        Some(workspace_selection_background(palette))
    } else if dragged {
        Some(palette.surface1)
    } else if focused {
        Some(workspace_active_background(palette, navigating))
    } else {
        None
    };
    if let Some(background) = background {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                buffer[(x, y)].set_bg(background);
            }
        }
    }
}
