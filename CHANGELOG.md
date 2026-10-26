# Changelog

## [1.1.0] - 2026-10-26

### Added
- `ui.pane_focus_passthrough` lists foreground programs, such as nvim, that
  get pane focus keys bound without the prefix (for example `ctrl+h`)
  instead of flockr. At its edge, nvim can call `flockr pane focus`; the
  README shows the setup.
- Clicking a flockr system notification on Linux switches to the agent's
  pane. On Hyprland it also brings the terminal window to the front.

### Fixed
- A stop signal sent right after the server started could kill it without
  a clean shutdown.

## [1.0.0] - 2026-10-18

First flockr release. flockr is a hard fork of
[herdr](https://github.com/herdrdev/herdr), taken from herdr's `master` at
[`143bf5fd`](https://github.com/herdrdev/herdr/commit/143bf5fd4781d164260c63fe28e7dc08054ad32f)
(after herdr v0.9.5). Changes before the fork are in
[herdr's changelog](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/CHANGELOG.md).
Version numbers start over at 1.0.0.

### Added
- Settings screen (`prefix+s`) in six sections: appearance, sidebar, alerts,
  behavior, session, and integrations. Changes save to the config file and
  apply at once.
- `ui.sidebar_position = "right"` puts the sidebar on the right edge.
- The sidebar has a title row with the menu, and All and Active tabs to filter
  the agents list.
- Agents are grouped under one header per space. A worktree's agents nest
  under their repo, as in the spaces list.
- `branch` agent row token: the agent's own branch, following it into the
  worktree it works in. `diff_stat` space row token: lines added and removed
  by uncommitted changes. Worktree spaces show ahead/behind and `diff_stat`.
- Agents and spaces are numbered in the sidebar, with `alt+1..9` and
  `alt+shift+1..9` to jump to them.
- `ui.seen_delay_ms` and `ui.seen_scope`: a done agent keeps its done badge
  until its pane has been on screen long enough. `prefix+a` jumps to the next
  agent that needs you, and `prefix+u` marks one done again.
- `ui.keep_agents_in_use_first`: a pane whose agent starts a fresh session
  moves behind the agents in use. Fresh sessions show a dotted ring, and
  `prefix+shift+s` sorts panes by age.
- Command palette (`prefix+:` or `prefix+/`) and vim jump keys in lists.
- Link hints (`prefix+f`) open or copy a visible link.
- tmux layout presets (`prefix+alt+1..8`, `prefix+space`) and tmux's split and
  resize keys.

### Changed
- Runs beside herdr: its own binary, config dir (`~/.config/flockr`), socket,
  `FLOCKR_*` environment variables, and agent hooks.
- Toasts stay beside the sidebar instead of covering it.
- Custom command popups are titled with their description.
- Resumed agents keep the CLI options they were started with. Codex runs
  without its shared daemon, so its hooks see the pane.

### Removed
- Self-update and background update checks.
- Downloading agent detection rules between releases
  (`flockr server update-agent-manifests`). Rules ship with each release.
- The sidebar's priority sort, replaced by the All and Active tabs.

### Compatibility
- Plugins and configs written for herdr work unchanged: `herdr-plugin.toml`,
  `min_herdr_version`, `HERDR_*` variables, and `delivery = "herdr"` are
  accepted.
