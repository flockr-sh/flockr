use crossterm::event::KeyCode;

use crate::config::{ActionKeybinds, CustomCommandKeybind, Keybinds};

use super::TerminalKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeybindDispatch {
    Direct,
    Prefix,
}

#[derive(Debug, Clone)]
pub(crate) enum KeybindMatch {
    Action(KeybindAction),
    Command(CustomCommandKeybind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeybindAction {
    NewWorkspace,
    NewWorktree,
    OpenWorktree,
    RemoveWorktree,
    RenameWorkspace,
    CloseWorkspace,
    SwitchWorkspace(usize),
    SwitchTab(usize),
    FocusAgent(usize),
    WorkspacePicker,
    PreviousWorkspace,
    NextWorkspace,
    PreviousAgent,
    NextAgent,
    NewTab,
    RenameTab,
    PreviousTab,
    NextTab,
    MoveTabPrevious,
    MoveTabNext,
    CloseTab,
    RenamePane,
    FocusPaneLeft,
    FocusPaneDown,
    FocusPaneUp,
    FocusPaneRight,
    SwapPaneLeft,
    SwapPaneDown,
    SwapPaneUp,
    SwapPaneRight,
    SplitVertical,
    SplitHorizontal,
    ClosePane,
    EditScrollback,
    ClearPane,
    CopyMode,
    LinkHints,
    Zoom,
    EnterResizeMode,
    ResizePaneLeft,
    ResizePaneDown,
    ResizePaneUp,
    ResizePaneRight,
    SelectLayout(crate::api::schema::LayoutSelection),
    SortPanesByAge,
    ToggleSidebar,
    CyclePaneNext,
    CyclePanePrevious,
    LastPane,
    Help,
    Settings,
    ReloadConfig,
    OpenNotificationTarget,
    FocusNextAttention,
    MarkUnread,
    CommandPalette,
    Detach,
    OpenNavigator,
}

pub(crate) fn resolve_direct_binding(
    keybinds: &Keybinds,
    key: &TerminalKey,
) -> Option<KeybindMatch> {
    resolve_exact_binding(keybinds, key, KeybindDispatch::Direct)
}

pub(crate) fn resolve_prefix_binding(
    keybinds: &Keybinds,
    key: &TerminalKey,
) -> Option<KeybindMatch> {
    resolve_exact_binding(keybinds, key, KeybindDispatch::Prefix).or_else(|| {
        generated_character_key(key).and_then(|generated_key| {
            resolve_exact_binding(keybinds, &generated_key, KeybindDispatch::Prefix)
        })
    })
}

/// Every non-indexed action with its bindings and a short label. Key
/// resolution and the command palette both read this one table, so a new
/// action is reachable from both once it is listed here.
pub(crate) fn action_table(
    keybinds: &Keybinds,
) -> Vec<(&ActionKeybinds, KeybindAction, &'static str)> {
    use crate::api::schema::LayoutSelection;

    vec![
        (&keybinds.help, KeybindAction::Help, "keybinds"),
        (&keybinds.settings, KeybindAction::Settings, "settings"),
        (
            &keybinds.command_palette,
            KeybindAction::CommandPalette,
            "command palette",
        ),
        (
            &keybinds.workspace_picker,
            KeybindAction::WorkspacePicker,
            "pick workspace",
        ),
        (
            &keybinds.new_workspace,
            KeybindAction::NewWorkspace,
            "new workspace",
        ),
        (
            &keybinds.new_worktree,
            KeybindAction::NewWorktree,
            "new worktree",
        ),
        (
            &keybinds.open_worktree,
            KeybindAction::OpenWorktree,
            "open worktree",
        ),
        (
            &keybinds.remove_worktree,
            KeybindAction::RemoveWorktree,
            "remove worktree",
        ),
        (
            &keybinds.rename_workspace,
            KeybindAction::RenameWorkspace,
            "rename workspace",
        ),
        (
            &keybinds.close_workspace,
            KeybindAction::CloseWorkspace,
            "close workspace",
        ),
        (
            &keybinds.previous_workspace,
            KeybindAction::PreviousWorkspace,
            "previous workspace",
        ),
        (
            &keybinds.next_workspace,
            KeybindAction::NextWorkspace,
            "next workspace",
        ),
        (
            &keybinds.previous_agent,
            KeybindAction::PreviousAgent,
            "previous agent",
        ),
        (&keybinds.next_agent, KeybindAction::NextAgent, "next agent"),
        (&keybinds.new_tab, KeybindAction::NewTab, "new tab"),
        (&keybinds.rename_tab, KeybindAction::RenameTab, "rename tab"),
        (
            &keybinds.previous_tab,
            KeybindAction::PreviousTab,
            "previous tab",
        ),
        (&keybinds.next_tab, KeybindAction::NextTab, "next tab"),
        (
            &keybinds.move_tab_previous,
            KeybindAction::MoveTabPrevious,
            "move tab left",
        ),
        (
            &keybinds.move_tab_next,
            KeybindAction::MoveTabNext,
            "move tab right",
        ),
        (&keybinds.close_tab, KeybindAction::CloseTab, "close tab"),
        (
            &keybinds.rename_pane,
            KeybindAction::RenamePane,
            "rename pane",
        ),
        (
            &keybinds.edit_scrollback,
            KeybindAction::EditScrollback,
            "edit scrollback in $EDITOR",
        ),
        (&keybinds.clear_pane, KeybindAction::ClearPane, "clear pane"),
        (&keybinds.copy_mode, KeybindAction::CopyMode, "copy mode"),
        (
            &keybinds.link_hints,
            KeybindAction::LinkHints,
            "open or copy a link",
        ),
        (
            &keybinds.focus_pane_left,
            KeybindAction::FocusPaneLeft,
            "focus pane left",
        ),
        (
            &keybinds.focus_pane_down,
            KeybindAction::FocusPaneDown,
            "focus pane down",
        ),
        (
            &keybinds.focus_pane_up,
            KeybindAction::FocusPaneUp,
            "focus pane up",
        ),
        (
            &keybinds.focus_pane_right,
            KeybindAction::FocusPaneRight,
            "focus pane right",
        ),
        (
            &keybinds.swap_pane_left,
            KeybindAction::SwapPaneLeft,
            "swap pane left",
        ),
        (
            &keybinds.swap_pane_down,
            KeybindAction::SwapPaneDown,
            "swap pane down",
        ),
        (
            &keybinds.swap_pane_up,
            KeybindAction::SwapPaneUp,
            "swap pane up",
        ),
        (
            &keybinds.swap_pane_right,
            KeybindAction::SwapPaneRight,
            "swap pane right",
        ),
        (&keybinds.last_pane, KeybindAction::LastPane, "last pane"),
        (
            &keybinds.cycle_pane_next,
            KeybindAction::CyclePaneNext,
            "next pane",
        ),
        (
            &keybinds.cycle_pane_previous,
            KeybindAction::CyclePanePrevious,
            "previous pane",
        ),
        (
            &keybinds.split_vertical,
            KeybindAction::SplitVertical,
            "split right",
        ),
        (
            &keybinds.split_horizontal,
            KeybindAction::SplitHorizontal,
            "split down",
        ),
        (&keybinds.close_pane, KeybindAction::ClosePane, "close pane"),
        (&keybinds.zoom, KeybindAction::Zoom, "zoom pane"),
        (
            &keybinds.resize_mode,
            KeybindAction::EnterResizeMode,
            "resize mode",
        ),
        (
            &keybinds.resize_pane_left,
            KeybindAction::ResizePaneLeft,
            "resize pane left",
        ),
        (
            &keybinds.resize_pane_down,
            KeybindAction::ResizePaneDown,
            "resize pane down",
        ),
        (
            &keybinds.resize_pane_up,
            KeybindAction::ResizePaneUp,
            "resize pane up",
        ),
        (
            &keybinds.resize_pane_right,
            KeybindAction::ResizePaneRight,
            "resize pane right",
        ),
        (
            &keybinds.layout_even_horizontal,
            KeybindAction::SelectLayout(LayoutSelection::EvenHorizontal),
            "layout: even horizontal",
        ),
        (
            &keybinds.layout_even_vertical,
            KeybindAction::SelectLayout(LayoutSelection::EvenVertical),
            "layout: even vertical",
        ),
        (
            &keybinds.layout_main_horizontal,
            KeybindAction::SelectLayout(LayoutSelection::MainHorizontal),
            "layout: main horizontal",
        ),
        (
            &keybinds.layout_main_vertical,
            KeybindAction::SelectLayout(LayoutSelection::MainVertical),
            "layout: main vertical",
        ),
        (
            &keybinds.layout_tiled,
            KeybindAction::SelectLayout(LayoutSelection::Tiled),
            "layout: tiled",
        ),
        (
            &keybinds.layout_main_horizontal_mirrored,
            KeybindAction::SelectLayout(LayoutSelection::MainHorizontalMirrored),
            "layout: main horizontal, mirrored",
        ),
        (
            &keybinds.layout_main_vertical_mirrored,
            KeybindAction::SelectLayout(LayoutSelection::MainVerticalMirrored),
            "layout: main vertical, mirrored",
        ),
        (
            &keybinds.layout_tiled_main_bottom,
            KeybindAction::SelectLayout(LayoutSelection::TiledMainBottom),
            "layout: tiled, main at bottom",
        ),
        (
            &keybinds.next_layout,
            KeybindAction::SelectLayout(LayoutSelection::Next),
            "next layout",
        ),
        (
            &keybinds.previous_layout,
            KeybindAction::SelectLayout(LayoutSelection::Previous),
            "previous layout",
        ),
        (
            &keybinds.spread_layout,
            KeybindAction::SelectLayout(LayoutSelection::Spread),
            "spread panes evenly",
        ),
        (
            &keybinds.sort_panes_by_age,
            KeybindAction::SortPanesByAge,
            "sort panes by age",
        ),
        (
            &keybinds.toggle_sidebar,
            KeybindAction::ToggleSidebar,
            "toggle sidebar",
        ),
        (
            &keybinds.reload_config,
            KeybindAction::ReloadConfig,
            "reload config",
        ),
        (
            &keybinds.open_notification_target,
            KeybindAction::OpenNotificationTarget,
            "open notification target",
        ),
        (
            &keybinds.focus_next_attention,
            KeybindAction::FocusNextAttention,
            "next agent that needs you",
        ),
        (
            &keybinds.mark_unread,
            KeybindAction::MarkUnread,
            "mark agent unread",
        ),
        (&keybinds.detach, KeybindAction::Detach, "detach"),
        (
            &keybinds.goto,
            KeybindAction::OpenNavigator,
            "go to (navigator)",
        ),
    ]
}

pub(crate) fn resolve_non_indexed_action(
    keybinds: &Keybinds,
    key: &TerminalKey,
    dispatch: KeybindDispatch,
) -> Option<KeybindAction> {
    action_table(keybinds)
        .into_iter()
        .find(|(bindings, _, _)| action_matches(bindings, key, dispatch))
        .map(|(_, action, _)| action)
}

pub(crate) fn resolve_custom_command(
    keybinds: &Keybinds,
    key: &TerminalKey,
    dispatch: KeybindDispatch,
) -> Option<CustomCommandKeybind> {
    keybinds
        .custom_commands
        .iter()
        .find(|binding| match dispatch {
            KeybindDispatch::Direct => binding.bindings.matches_direct_key(key),
            KeybindDispatch::Prefix => binding.bindings.matches_prefix_key(key),
        })
        .cloned()
}

pub(crate) fn resolve_indexed_action(
    keybinds: &Keybinds,
    key: &TerminalKey,
    dispatch: KeybindDispatch,
) -> Option<KeybindAction> {
    let actual_modifiers = crate::config::normalize_key_combo((key.code, key.modifiers)).1;

    for exact_modifiers in [true, false] {
        let trigger_matches = |binding: &crate::config::IndexedKeybind| {
            let dispatch_matches = match dispatch {
                KeybindDispatch::Direct => binding.trigger.is_direct(),
                KeybindDispatch::Prefix => binding.trigger.is_prefix(),
            };
            let expected_modifiers = crate::config::normalize_key_combo(binding.trigger.combo()).1;
            dispatch_matches && (actual_modifiers == expected_modifiers) == exact_modifiers
        };

        for binding in &keybinds.switch_tab {
            if trigger_matches(binding) {
                if let Some(index) = binding.matched_index(key) {
                    return Some(KeybindAction::SwitchTab(index));
                }
            }
        }
        for binding in &keybinds.switch_workspace {
            if trigger_matches(binding) {
                if let Some(index) = binding.matched_index(key) {
                    return Some(KeybindAction::SwitchWorkspace(index));
                }
            }
        }
        for binding in &keybinds.focus_agent {
            if trigger_matches(binding) {
                if let Some(index) = binding.matched_index(key) {
                    return Some(KeybindAction::FocusAgent(index));
                }
            }
        }
    }

    None
}

fn resolve_exact_binding(
    keybinds: &Keybinds,
    key: &TerminalKey,
    dispatch: KeybindDispatch,
) -> Option<KeybindMatch> {
    resolve_non_indexed_action(keybinds, key, dispatch)
        .map(KeybindMatch::Action)
        .or_else(|| resolve_custom_command(keybinds, key, dispatch).map(KeybindMatch::Command))
        .or_else(|| resolve_indexed_action(keybinds, key, dispatch).map(KeybindMatch::Action))
}

fn generated_character_key(key: &TerminalKey) -> Option<TerminalKey> {
    let mut characters = key.generated_text.as_deref()?.chars();
    let character = characters.next()?;
    if character.is_control() || characters.next().is_some() {
        return None;
    }
    Some(TerminalKey::new(
        KeyCode::Char(character),
        crossterm::event::KeyModifiers::empty(),
    ))
}

fn action_matches(
    bindings: &crate::config::ActionKeybinds,
    key: &TerminalKey,
    dispatch: KeybindDispatch,
) -> bool {
    match dispatch {
        KeybindDispatch::Direct => bindings.matches_direct_key(key),
        KeybindDispatch::Prefix => bindings.matches_prefix_key(key),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::*;

    #[test]
    fn clear_pane_is_unbound_by_default_and_configurable() {
        assert!(crate::config::Config::default()
            .keybinds()
            .clear_pane
            .bindings
            .is_empty());
        let config: crate::config::Config =
            toml::from_str("[keys]\nclear_pane = [\"super+k\", \"prefix+ctrl+k\"]").unwrap();
        assert!(config.collect_diagnostics().is_empty());
        let keybinds = config.keybinds();
        assert!(matches!(
            resolve_direct_binding(
                &keybinds,
                &TerminalKey::new(KeyCode::Char('k'), KeyModifiers::SUPER)
            ),
            Some(KeybindMatch::Action(KeybindAction::ClearPane))
        ));
        assert!(matches!(
            resolve_prefix_binding(
                &keybinds,
                &TerminalKey::new(KeyCode::Char('k'), KeyModifiers::CONTROL)
            ),
            Some(KeybindMatch::Action(KeybindAction::ClearPane))
        ));
        assert!(matches!(
            resolve_prefix_binding(
                &keybinds,
                &TerminalKey::new(KeyCode::Char('k'), KeyModifiers::SHIFT)
            ),
            Some(KeybindMatch::Action(KeybindAction::SwapPaneUp))
        ));
    }

    #[test]
    fn one_shared_resolver_handles_direct_prefix_and_indexed_bindings() {
        let keybinds = Keybinds {
            next_tab: crate::config::ActionKeybinds::direct("ctrl+n"),
            ..Keybinds::default()
        };

        let direct = TerminalKey::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert!(matches!(
            resolve_direct_binding(&keybinds, &direct),
            Some(KeybindMatch::Action(KeybindAction::NextTab))
        ));

        let help = TerminalKey::new(KeyCode::Char('?'), KeyModifiers::empty());
        assert!(matches!(
            resolve_prefix_binding(&keybinds, &help),
            Some(KeybindMatch::Action(KeybindAction::Help))
        ));

        let one = TerminalKey::new(KeyCode::Char('1'), KeyModifiers::empty());
        assert!(matches!(
            resolve_prefix_binding(&keybinds, &one),
            Some(KeybindMatch::Action(KeybindAction::SwitchTab(0)))
        ));
    }

    #[test]
    fn prefix_resolution_uses_shared_generated_character_fallback() {
        let keybinds = Keybinds::default();
        let key = TerminalKey::new(KeyCode::Char('/'), KeyModifiers::SHIFT)
            .with_generated_text(Some("?".to_owned()));

        assert!(matches!(
            resolve_prefix_binding(&keybinds, &key),
            Some(KeybindMatch::Action(KeybindAction::Help))
        ));
    }
}
