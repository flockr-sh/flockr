use super::*;

/// One runnable row in the command palette.
#[derive(Debug, Clone)]
pub(super) struct ClientCommandPaletteEntry {
    pub(super) label: String,
    /// The bound keys, or empty when the action has none.
    pub(super) keys: String,
    pub(super) binding: crate::input::KeybindMatch,
}

/// Every action and custom command the palette can run, in table order.
pub(super) fn command_palette_entries(
    keybinds: &crate::config::Keybinds,
) -> Vec<ClientCommandPaletteEntry> {
    let mut entries: Vec<_> = crate::input::action_table(keybinds)
        .into_iter()
        // The palette is already open, so listing itself would do nothing.
        .filter(|(_, action, _)| *action != crate::input::KeybindAction::CommandPalette)
        .map(|(bindings, action, label)| ClientCommandPaletteEntry {
            label: label.to_owned(),
            keys: bindings.label().unwrap_or_default(),
            binding: crate::input::KeybindMatch::Action(action),
        })
        .collect();
    entries.extend(keybinds.custom_commands.iter().map(|command| {
        ClientCommandPaletteEntry {
            // Clients only see an opaque command id, so a command without a
            // description is named by its kind; its keys tell it apart.
            label: command.description.clone().unwrap_or_else(|| {
                match command.action {
                    crate::config::CustomCommandAction::PluginAction => "plugin action",
                    _ => "custom command",
                }
                .to_owned()
            }),
            keys: command.bindings.label().unwrap_or_default(),
            binding: crate::input::KeybindMatch::Command(command.clone()),
        }
    }));
    entries
}

/// How well `query` matches `text`, lower is better. `None` means no match.
fn match_rank(text: &str, query: &str) -> Option<u8> {
    let text = text.to_lowercase();
    if text == query {
        Some(0)
    } else if text.starts_with(query) {
        Some(1)
    } else if text
        .match_indices(query)
        .any(|(index, _)| index == 0 || !text[..index].ends_with(char::is_alphanumeric))
    {
        Some(2)
    } else if text.contains(query) {
        Some(3)
    } else {
        let mut remaining = text.chars();
        query
            .chars()
            .filter(|character| !character.is_whitespace())
            .all(|wanted| remaining.any(|character| character == wanted))
            .then_some(4)
    }
}

/// Indices into `entries` that match `query`, best match first. Ties keep
/// table order, and an empty query lists everything.
pub(super) fn filter_command_palette(
    entries: &[ClientCommandPaletteEntry],
    query: &str,
) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..entries.len()).collect();
    }
    let mut ranked: Vec<(u8, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let label = match_rank(&entry.label, &query);
            // A shortcut matches only as typed text, never as a scattered
            // subsequence, so "x" does not pull in every "prefix+..." row.
            let keys = match_rank(&entry.keys, &query).filter(|rank| *rank < 4);
            label
                .into_iter()
                .chain(keys)
                .min()
                .map(|rank| (rank, index))
        })
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, index)| index).collect()
}

impl ClientShellState {
    pub(super) fn open_command_palette(&mut self) {
        self.overlay = Some(ClientShellOverlay::CommandPalette(
            ClientCommandPaletteOverlay {
                entries: command_palette_entries(&self.config.keybinds.keybinds),
                query: TextEditor::default(),
                selected: 0,
            },
        ));
    }

    pub(super) fn move_command_palette_selection(&mut self, delta: isize) {
        let Some(ClientShellOverlay::CommandPalette(palette)) = self.overlay.as_mut() else {
            return;
        };
        let count = palette.filtered().len();
        palette.selected =
            (palette.selected as isize + delta).clamp(0, count.saturating_sub(1) as isize) as usize;
    }

    /// Runs the row at `position` in the filtered list, as if its key had
    /// been pressed.
    pub(super) fn activate_command_palette_row(
        &mut self,
        position: usize,
        outcome: &mut ClientShellInput,
    ) {
        let Some(ClientShellOverlay::CommandPalette(palette)) = self.overlay.as_ref() else {
            return;
        };
        let Some(binding) = palette
            .filtered()
            .get(position)
            .map(|index| palette.entries[*index].binding.clone())
        else {
            return;
        };
        self.overlay = None;
        self.record_binding(binding, outcome);
        outcome.repaint = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str, keys: &str) -> ClientCommandPaletteEntry {
        ClientCommandPaletteEntry {
            label: label.into(),
            keys: keys.into(),
            binding: crate::input::KeybindMatch::Action(crate::input::KeybindAction::Help),
        }
    }

    fn labels<'a>(entries: &'a [ClientCommandPaletteEntry], query: &str) -> Vec<&'a str> {
        filter_command_palette(entries, query)
            .into_iter()
            .map(|index| entries[index].label.as_str())
            .collect()
    }

    #[test]
    fn ranks_exact_then_prefix_then_word_start_then_substring_then_subsequence() {
        let entries = [
            entry("rename tab", ""),
            entry("close tab", ""),
            entry("new tab", ""),
            entry("tab", ""),
            entry("previous tab", ""),
            entry("stable", ""),
            entry("toggle a bar", ""),
        ];
        assert_eq!(
            labels(&entries, "tab"),
            [
                "tab",
                "rename tab",
                "close tab",
                "new tab",
                "previous tab",
                "stable",
                "toggle a bar",
            ]
        );
        assert_eq!(labels(&entries, "new tab"), ["new tab"]);
        assert_eq!(labels(&entries, "nt"), ["rename tab", "new tab"]);
    }

    #[test]
    fn matches_typed_shortcuts_but_not_scattered_ones() {
        let entries = [
            entry("close pane", "prefix+x"),
            entry("zoom pane", "prefix+z"),
        ];
        assert_eq!(labels(&entries, "prefix+x"), ["close pane"]);
        assert_eq!(labels(&entries, "pfxz"), Vec::<&str>::new());
    }

    #[test]
    fn empty_query_lists_everything_in_table_order() {
        let entries = [entry("b", ""), entry("a", "")];
        assert_eq!(labels(&entries, "  "), ["b", "a"]);
    }

    #[test]
    fn lists_custom_commands_and_leaves_out_itself() {
        let config: crate::config::Config = toml::from_str(
            r#"
[[keys.command]]
key = "prefix+alt+g"
description = "git ui"
command = "lazygit"
type = "popup"

[[keys.command]]
key = "prefix+alt+t"
command = "htop"
type = "popup"
"#,
        )
        .unwrap();
        let entries = command_palette_entries(&config.keybinds());
        assert!(entries.iter().any(|entry| entry.label == "git ui"
            && entry.keys == "prefix+alt+g"
            && matches!(entry.binding, crate::input::KeybindMatch::Command(_))));
        assert!(entries
            .iter()
            .any(|entry| entry.label == "custom command" && entry.keys == "prefix+alt+t"));
        assert!(entries.iter().any(|entry| entry.label == "close pane"));
        assert!(!entries.iter().any(|entry| matches!(
            entry.binding,
            crate::input::KeybindMatch::Action(crate::input::KeybindAction::CommandPalette)
        )));
    }
}
