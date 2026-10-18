//! The settings screen's rows: which config key each one edits, the values
//! it offers, and how to read its current value from a loaded config.

use super::ClientSettingsSection;
use crate::config::Config;

/// A TOML value a row can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingValue {
    Str(&'static str),
    Bool(bool),
    Int(i64),
}

impl SettingValue {
    /// The value as it is written after `key = ` in the config file.
    pub(super) fn literal(self) -> String {
        match self {
            Self::Str(value) => format!("\"{value}\""),
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
        }
    }

    fn matches(self, value: &toml::Value) -> bool {
        match (self, value) {
            (Self::Str(choice), toml::Value::String(value)) => {
                super::settings::normalized_theme_name(choice)
                    == super::settings::normalized_theme_name(value)
            }
            (Self::Bool(choice), toml::Value::Boolean(value)) => choice == *value,
            (Self::Int(choice), toml::Value::Integer(value)) => choice == *value,
            _ => false,
        }
    }
}

pub(super) enum SettingChoices {
    Fixed(&'static [(&'static str, SettingValue)]),
    Themes,
}

pub(super) struct SettingRow {
    pub(super) section: ClientSettingsSection,
    /// TOML table the key lives in, such as `ui.sidebar`.
    pub(super) table: &'static str,
    pub(super) key: &'static str,
    pub(super) label: &'static str,
    pub(super) help: &'static str,
    choices: SettingChoices,
    current: fn(&Config) -> toml::Value,
    /// Rows that only matter while another setting is on.
    visible: fn(&Config) -> bool,
}

impl SettingRow {
    pub(super) fn choice_count(&self) -> usize {
        match self.choices {
            SettingChoices::Fixed(choices) => choices.len(),
            SettingChoices::Themes => crate::config::THEME_NAMES.len(),
        }
    }

    pub(super) fn choice(&self, index: usize) -> Option<(&'static str, SettingValue)> {
        match self.choices {
            SettingChoices::Fixed(choices) => choices.get(index).copied(),
            SettingChoices::Themes => crate::config::THEME_NAMES
                .get(index)
                .map(|name| (*name, SettingValue::Str(name))),
        }
    }

    /// Index of the config's current value among the choices, or `None` when
    /// the file holds a value the row doesn't offer.
    pub(super) fn current_index(&self, config: &Config) -> Option<usize> {
        let current = (self.current)(config);
        (0..self.choice_count()).find(|index| {
            self.choice(*index)
                .is_some_and(|(_, value)| value.matches(&current))
        })
    }

    /// Label for the current value, or the raw value when no choice matches.
    pub(super) fn current_label(&self, config: &Config) -> String {
        match self
            .current_index(config)
            .and_then(|index| self.choice(index))
        {
            Some((label, _)) => label.to_owned(),
            None => match (self.current)(config) {
                toml::Value::String(value) => value,
                value => value.to_string(),
            },
        }
    }

    /// The value one step from the current one, wrapping at either end.
    pub(super) fn step(&self, config: &Config, delta: isize) -> Option<SettingValue> {
        let count = self.choice_count();
        if count == 0 {
            return None;
        }
        let next = match self.current_index(config) {
            Some(index) => (index as isize + delta).rem_euclid(count as isize) as usize,
            None => 0,
        };
        self.choice(next).map(|(_, value)| value)
    }
}

pub(super) fn visible_rows(
    section: ClientSettingsSection,
    config: &Config,
) -> impl Iterator<Item = &'static SettingRow> + '_ {
    ROWS.iter()
        .filter(move |row| row.section == section && (row.visible)(config))
}

const ON_OFF: &[(&str, SettingValue)] = &[
    ("on", SettingValue::Bool(true)),
    ("off", SettingValue::Bool(false)),
];

fn always(_: &Config) -> bool {
    true
}

fn value<T: serde::Serialize>(value: &T) -> toml::Value {
    toml::Value::try_from(value).unwrap_or(toml::Value::String(String::new()))
}

fn theme_value(name: Option<&str>, default: &str) -> toml::Value {
    toml::Value::String(name.unwrap_or(default).to_owned())
}

use ClientSettingsSection::{Alerts, Appearance, Behavior, Session, Sidebar};
use SettingValue::{Int, Str};

static ROWS: &[SettingRow] = &[
    SettingRow {
        section: Appearance,
        table: "theme",
        key: "name",
        label: "theme",
        help: "Colors for flockr's own UI. Not used while following light/dark.",
        choices: SettingChoices::Themes,
        current: |c| theme_value(c.theme.name.as_deref(), "catppuccin"),
        visible: |c| !c.theme.auto_switch,
    },
    SettingRow {
        section: Appearance,
        table: "theme",
        key: "auto_switch",
        label: "follow light/dark",
        help: "Switch between a dark and a light theme with the terminal's appearance.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.theme.auto_switch),
        visible: always,
    },
    SettingRow {
        section: Appearance,
        table: "theme",
        key: "dark_name",
        label: "dark theme",
        help: "Theme used while the terminal is dark.",
        choices: SettingChoices::Themes,
        current: |c| theme_value(c.theme.dark_name.as_deref(), "catppuccin"),
        visible: |c| c.theme.auto_switch,
    },
    SettingRow {
        section: Appearance,
        table: "theme",
        key: "light_name",
        label: "light theme",
        help: "Theme used while the terminal is light.",
        choices: SettingChoices::Themes,
        current: |c| theme_value(c.theme.light_name.as_deref(), "catppuccin-latte"),
        visible: |c| c.theme.auto_switch,
    },
    SettingRow {
        section: Appearance,
        table: "ui",
        key: "status_indicators",
        label: "status indicators",
        help: "Color dots, or a distinct symbol for each agent state.",
        choices: SettingChoices::Fixed(&[
            ("dots  ● ● ● ○ ·", Str("dots")),
            ("symbols  × ◐ ✓ ○ ·", Str("symbols")),
        ]),
        current: |c| value(&c.ui.status_indicators),
        visible: always,
    },
    SettingRow {
        section: Appearance,
        table: "ui",
        key: "pulse_working",
        label: "pulse working",
        help: "Pulse the status mark while an agent works.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.pulse_working),
        visible: always,
    },
    SettingRow {
        section: Appearance,
        table: "ui",
        key: "pane_borders",
        label: "pane borders",
        help: "auto frames split panes; always also frames a lone pane.",
        choices: SettingChoices::Fixed(&[
            ("auto", Str("auto")),
            ("always", Str("always")),
            ("off", Str("off")),
        ]),
        current: |c| {
            toml::Value::String(
                match c.ui.pane_borders {
                    crate::config::PaneBordersConfig::Auto => "auto",
                    crate::config::PaneBordersConfig::Always => "always",
                    crate::config::PaneBordersConfig::Off => "off",
                }
                .to_owned(),
            )
        },
        visible: always,
    },
    SettingRow {
        section: Appearance,
        table: "ui",
        key: "pane_outer_borders",
        label: "outer borders",
        help: "Draw borders along the outside edge of the pane area.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.pane_outer_borders),
        visible: always,
    },
    SettingRow {
        section: Appearance,
        table: "ui",
        key: "pane_gaps",
        label: "pane gaps",
        help: "Keep split panes apart instead of sharing one divider.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.pane_gaps),
        visible: always,
    },
    SettingRow {
        section: Appearance,
        table: "ui",
        key: "pane_scrollbars",
        label: "pane scrollbars",
        help: "Draw a scrollbar beside each pane.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.pane_scrollbars),
        visible: always,
    },
    SettingRow {
        section: Appearance,
        table: "ui",
        key: "show_agent_labels_on_pane_borders",
        label: "agent names on borders",
        help: "Show the detected agent in a split pane's border when the pane has no name.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.show_agent_labels_on_pane_borders),
        visible: always,
    },
    SettingRow {
        section: Appearance,
        table: "ui",
        key: "tab_bar_position",
        label: "tab bar",
        help: "Where the tab row sits.",
        choices: SettingChoices::Fixed(&[("top", Str("top")), ("bottom", Str("bottom"))]),
        current: |c| value(&c.ui.tab_bar_position),
        visible: always,
    },
    SettingRow {
        section: Appearance,
        table: "ui",
        key: "hide_tab_bar_when_single_tab",
        label: "hide single tab",
        help: "Hide the tab row when a space has one tab.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.hide_tab_bar_when_single_tab),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui",
        key: "sidebar_position",
        label: "position",
        help: "Screen edge the sidebar sits on.",
        choices: SettingChoices::Fixed(&[("left", Str("left")), ("right", Str("right"))]),
        current: |c| value(&c.ui.sidebar_position),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui",
        key: "sidebar_collapsed_mode",
        label: "when collapsed",
        help: "compact keeps a narrow status rail; hidden takes no space.",
        choices: SettingChoices::Fixed(&[("compact", Str("compact")), ("hidden", Str("hidden"))]),
        current: |c| value(&c.ui.sidebar_collapsed_mode),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui",
        key: "sidebar_start_collapsed",
        label: "start collapsed",
        help: "Start with the sidebar collapsed. Applies on the next launch.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.sidebar_start_collapsed),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui.sidebar",
        key: "header",
        label: "section headers",
        help: "band fills the header row; plain is bold text.",
        choices: SettingChoices::Fixed(&[("band", Str("band")), ("plain", Str("plain"))]),
        current: |c| value(&c.ui.sidebar.header),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui.sidebar",
        key: "header_counts",
        label: "header counts",
        help: "Show how many spaces, machines, or agents each header holds.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.sidebar.header_counts),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui.sidebar",
        key: "divider",
        label: "divider",
        help: "Line between spaces and agents. handle shows a drag grip.",
        choices: SettingChoices::Fixed(&[
            ("handle", Str("handle")),
            ("line", Str("line")),
            ("dim", Str("dim")),
        ]),
        current: |c| value(&c.ui.sidebar.divider),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui.sidebar",
        key: "buttons",
        label: "buttons",
        help: "chip draws filled labels; text draws plain ones.",
        choices: SettingChoices::Fixed(&[("chip", Str("chip")), ("text", Str("text"))]),
        current: |c| value(&c.ui.sidebar.buttons),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui.sidebar",
        key: "grab",
        label: "grabbed space",
        help: "How navigate mode marks a space you are moving.",
        choices: SettingChoices::Fixed(&[("marker", Str("marker")), ("fill", Str("fill"))]),
        current: |c| value(&c.ui.sidebar.grab),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui.sidebar.spaces",
        key: "row_gap",
        label: "space spacing",
        help: "Blank rows between spaces.",
        choices: SettingChoices::Fixed(&[("0", Int(0)), ("1", Int(1))]),
        current: |c| value(&c.ui.sidebar.spaces.row_gap),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui.sidebar.spaces",
        key: "border",
        label: "space border",
        help: "Mark where each space ends.",
        choices: SettingChoices::Fixed(&[
            ("none", Str("none")),
            ("underline", Str("underline")),
            ("bracket", Str("bracket")),
        ]),
        current: |c| value(&c.ui.sidebar.spaces.border),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui.sidebar.agents",
        key: "row_gap",
        label: "agent spacing",
        help: "Blank rows between agents.",
        choices: SettingChoices::Fixed(&[("0", Int(0)), ("1", Int(1))]),
        current: |c| value(&c.ui.sidebar.agents.row_gap),
        visible: always,
    },
    SettingRow {
        section: Sidebar,
        table: "ui.sidebar.agents",
        key: "border",
        label: "agent border",
        help: "Mark where each space's agents end.",
        choices: SettingChoices::Fixed(&[
            ("none", Str("none")),
            ("underline", Str("underline")),
            ("bracket", Str("bracket")),
        ]),
        current: |c| value(&c.ui.sidebar.agents.border),
        visible: always,
    },
    SettingRow {
        section: Alerts,
        table: "ui.toast",
        key: "delivery",
        label: "popups",
        help: "Where background notifications appear.",
        choices: SettingChoices::Fixed(&[
            ("off", Str("off")),
            ("inside flockr", Str("flockr")),
            ("via terminal", Str("terminal")),
            ("via system", Str("system")),
        ]),
        current: |c| value(&c.ui.toast.delivery),
        visible: always,
    },
    SettingRow {
        section: Alerts,
        table: "ui.toast.flockr",
        key: "position",
        label: "popup position",
        help: "Corner for popups shown inside flockr.",
        choices: SettingChoices::Fixed(&[
            ("bottom right", Str("bottom-right")),
            ("bottom left", Str("bottom-left")),
            ("top right", Str("top-right")),
            ("top left", Str("top-left")),
        ]),
        current: |c| value(&c.ui.toast.flockr.position),
        visible: |c| c.ui.toast.delivery == crate::config::ToastDelivery::Flockr,
    },
    SettingRow {
        section: Alerts,
        table: "ui.toast",
        key: "delay_seconds",
        label: "popup delay",
        help: "Seconds to wait before a popup, so quick changes don't show one.",
        choices: SettingChoices::Fixed(&[
            ("0s", Int(0)),
            ("1s", Int(1)),
            ("3s", Int(3)),
            ("5s", Int(5)),
            ("10s", Int(10)),
        ]),
        current: |c| value(&c.ui.toast.delay_seconds),
        visible: |c| c.ui.toast.delivery != crate::config::ToastDelivery::Off,
    },
    SettingRow {
        section: Alerts,
        table: "ui.sound",
        key: "enabled",
        label: "sound",
        help: "Play a sound when agents in the background change state.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.sound.enabled),
        visible: always,
    },
    SettingRow {
        section: Alerts,
        table: "ui.toast.clipboard",
        key: "enabled",
        label: "copy notice",
        help: "Show a short notice when text is copied.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.toast.clipboard.enabled),
        visible: always,
    },
    SettingRow {
        section: Alerts,
        table: "ui",
        key: "seen_delay_ms",
        label: "done badge delay",
        help: "How long a finished agent stays on screen before its done badge clears.",
        choices: SettingChoices::Fixed(&[
            ("none", Int(0)),
            ("0.5s", Int(500)),
            ("1s", Int(1000)),
            ("2s", Int(2000)),
            ("5s", Int(5000)),
        ]),
        current: |c| value(&c.ui.seen_delay_ms),
        visible: always,
    },
    SettingRow {
        section: Alerts,
        table: "ui",
        key: "seen_scope",
        label: "done badge clears",
        help: "pane clears it when you focus the agent's pane; tab when you open its tab.",
        choices: SettingChoices::Fixed(&[("pane", Str("pane")), ("tab", Str("tab"))]),
        current: |c| value(&c.ui.seen_scope),
        visible: always,
    },
    SettingRow {
        section: Behavior,
        table: "ui",
        key: "mouse_capture",
        label: "mouse",
        help: "Off lets the terminal handle clicks, such as opening links.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.mouse_capture),
        visible: always,
    },
    SettingRow {
        section: Behavior,
        table: "ui",
        key: "copy_on_select",
        label: "copy on select",
        help: "Copy text as soon as it is selected with the mouse.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.copy_on_select),
        visible: always,
    },
    SettingRow {
        section: Behavior,
        table: "ui",
        key: "mouse_scroll_lines",
        label: "wheel scroll",
        help: "Lines to scroll per mouse wheel notch.",
        choices: SettingChoices::Fixed(&[
            ("1 line", Int(1)),
            ("3 lines", Int(3)),
            ("5 lines", Int(5)),
            ("10 lines", Int(10)),
        ]),
        current: |c| value(&(c.ui.mouse_scroll_lines() as i64)),
        visible: always,
    },
    SettingRow {
        section: Behavior,
        table: "ui",
        key: "confirm_close",
        label: "confirm close",
        help: "Ask before closing a space.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.confirm_close),
        visible: always,
    },
    SettingRow {
        section: Behavior,
        table: "ui",
        key: "prompt_new_tab_name",
        label: "name new tabs",
        help: "Ask for a name when creating a tab.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.prompt_new_tab_name),
        visible: always,
    },
    SettingRow {
        section: Behavior,
        table: "ui",
        key: "prompt_new_workspace_name",
        label: "name new spaces",
        help: "Ask for a name when creating a space.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.prompt_new_workspace_name),
        visible: always,
    },
    SettingRow {
        section: Behavior,
        table: "terminal",
        key: "new_cwd",
        label: "new pane folder",
        help: "follow uses the current pane's folder. A fixed path can be set in the config file.",
        choices: SettingChoices::Fixed(&[
            ("follow", Str("follow")),
            ("home", Str("home")),
            ("flockr's folder", Str("current")),
        ]),
        current: |c| {
            toml::Value::String(match &c.terminal.new_cwd {
                crate::config::NewTerminalCwdConfig::Follow => "follow".to_owned(),
                crate::config::NewTerminalCwdConfig::Home => "home".to_owned(),
                crate::config::NewTerminalCwdConfig::Current => "current".to_owned(),
                crate::config::NewTerminalCwdConfig::Path(path) => path.clone(),
            })
        },
        visible: always,
    },
    SettingRow {
        section: Behavior,
        table: "ui",
        key: "keep_agents_in_use_first",
        label: "agents in use first",
        help: "Move a pane behind the agents in use when its agent starts a fresh session.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.ui.keep_agents_in_use_first),
        visible: always,
    },
    SettingRow {
        section: Session,
        table: "session",
        key: "resume_agents_on_restore",
        label: "resume agents",
        help: "Resume agent conversations after a server restart. Needs the agent's integration.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.session.resume_agents_on_restore),
        visible: always,
    },
    SettingRow {
        section: Session,
        table: "experimental",
        key: "pane_history",
        label: "save pane history",
        help: "Keep recent pane output across full server restarts.",
        choices: SettingChoices::Fixed(ON_OFF),
        current: |c| value(&c.experimental.pane_history),
        visible: always,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row writes a value its own key parses, and a config holding that
    /// value reads back as the same choice.
    #[test]
    fn every_choice_round_trips_through_the_config_file() {
        for row in ROWS {
            for index in 0..row.choice_count() {
                let (_, choice) = row.choice(index).unwrap();
                let toml = format!("[{}]\n{} = {}\n", row.table, row.key, choice.literal());
                let config: Config = toml::from_str(&toml)
                    .unwrap_or_else(|error| panic!("{} {}: {error}", row.table, row.key));
                assert_eq!(
                    row.current_index(&config),
                    Some(index),
                    "{} {} = {}",
                    row.table,
                    row.key,
                    choice.literal()
                );
            }
        }
    }

    #[test]
    fn every_row_shows_its_default_as_one_of_its_choices() {
        let config = Config::default();
        for row in ROWS {
            assert!(
                row.current_index(&config).is_some(),
                "{} {} default is not a choice",
                row.table,
                row.key
            );
        }
    }

    #[test]
    fn a_value_outside_the_choices_shows_as_is_and_steps_to_the_first() {
        let config: Config = toml::from_str("[terminal]\nnew_cwd = \"~/work\"\n").unwrap();
        let row = ROWS.iter().find(|row| row.key == "new_cwd").unwrap();
        assert_eq!(row.current_label(&config), "~/work");
        assert_eq!(row.step(&config, 1), Some(Str("follow")));
    }

    #[test]
    fn light_and_dark_theme_rows_replace_the_theme_row_while_following() {
        let keys = |config: &Config| {
            visible_rows(Appearance, config)
                .map(|row| row.key)
                .take(4)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            keys(&Config::default()),
            ["name", "auto_switch", "status_indicators", "pulse_working"]
        );
        let following: Config = toml::from_str("[theme]\nauto_switch = true\n").unwrap();
        assert_eq!(
            keys(&following),
            [
                "auto_switch",
                "dark_name",
                "light_name",
                "status_indicators"
            ]
        );
    }
}
