# Changelog

## [1.3.0] - 2026-11-04

### Fixed
- `flockr pane move` no longer refuses to move a pane into or out of a
  zoomed tab. The source tab stays zoomed, on the pane that takes focus
  if the zoomed pane left. In a zoomed target tab, `--no-focus` adds the
  pane behind the zoom and `--focus` ends the zoom.
- Closing the zoomed pane keeps the tab zoomed on the pane that takes
  focus, instead of showing every pane. Closing or moving out panes ends
  the zoom only when one pane is left.
- `flockr pane split --no-focus` in a zoomed tab opens the new pane
  behind the zoom instead of ending it. A focused split still ends it.
- A phone or web app showing a pane in Fit no longer locks the pane's
  size. The device used last sets the size; the others show the pane
  scaled to width with a notice. Opening the pane, tapping or clicking
  it, or typing into it takes the size. In the TUI, switching to the tab
  takes back every pane in it, and typing, pasting, or clicking in a pane
  takes back that pane. Focusing a window, moving the mouse, scrolling,
  and reconnecting do not, except that a device whose connection dropped
  takes the size back if nobody used the pane meanwhile.
- `flockr pane move` exits 1 when the move changes nothing for any reason
  but `same_tab`.

### Changed
- The phone app's notification banner matches the agent cards: status
  dot, one-line title and body, and a close button.
- The flockr skill tells agents to close their own pane, as their last
  step, when told to close or kill themselves when done, and shows
  `pane move` examples. Reinstall or update the Claude and Codex
  integrations to get it.

## [1.2.0] - 2026-11-03

### Added
- Mobile gateway for the flockr phone app, off by default (`[mobile]` in
  the config). `flockr mobile pair` shows a QR code; the phone then lists
  agents and terminals, streams panes, types into them, and gets alerts
  when an agent is blocked or done. `flockr mobile devices` lists and
  revokes phones, and `flockr mobile status` shows the gateway.
- Every phone connection is end-to-end encrypted with Noise and pinned to
  the server key in the QR code. There is no plain-text mode.
- `pane.mark_read` API method. Reading a finished agent on any desktop
  client or phone now clears its done badge everywhere.
- `agent.list` and `agent.get` report the agent's `branch` and whether its
  session is still empty (`session_empty`).
- The phone app can attach images. The gateway saves each one like a
  desktop clipboard image paste and pastes its path into the pane.
- Flock: several machines' flockr servers act as one, off by default
  (`[flock]` in the config). Start one with `flockr flock init` on the hub,
  add machines with `flockr flock invite` there and `flockr flock join` on
  the new machine; no SSH needed. Members connect only to the hub, over
  Noise, with keys the hub checks against its member list. Roles are
  admin, member, and viewer. `flockr flock agents` lists every agent with
  the address to send it a message, and `flockr --machine <name>` runs
  commands on another member.
- The TUI shows other flock members' panes, and the phone app shows every
  machine in the flock through the one it paired with.
- Agents message each other on any machine: `flockr message send
  machine:agent "..."`, with `--wait` for the reply, and `flockr message
  read` and `reply`. An agent can be addressed by name, pane ID, or kind
  (`tower:claude`) when only one of that kind runs there. flockr types a
  notice into the recipient's pane when it is idle, and holds it while a
  person is typing there. Senders are verified, conversations stop after 8
  replies, and every message is logged in `messages.jsonl`.
- `[flock] approve_messages` decides which agent messages wait for a
  person: `other-projects` (default: same-repo messages and replies go
  through), `never`, or `always`. Approve or deny with `flockr message
  approve` and `deny`, from the phone or web app, or allow a pair for
  good. The setting is in the TUI settings and the apps, and applies
  without a restart.
- Web app, off by default (`[web]` in the config): the phone app's screens
  in a desktop layout, served over plain HTTP on your private network.
  Type straight into a terminal, paste text and images, approve messages,
  and change the approval setting. It ships inside flockr: `[web] enabled
  = true` serves it with no separate build, and `[web] dir` serves another
  build instead.
- `flockr integration install claude` and `codex` also install the flockr
  skill, which tells agents how to message each other and to act on the
  messages they receive.
- A flockr theme, dark (`flockr`) and light (`flockr-light`), in the
  colors of the phone app. With `auto_switch` on, they pair with each
  other.
- Blocked alerts on the phone show the agent's question and command
  ("Do you want to proceed? · npm test") and, for plain Yes / No prompts,
  carry answer buttons: long-press for Yes, Yes don't ask again, No, or
  Open. The app checks the prompt is unchanged before it answers.
  `[mobile.push] alert_prompt_text = false` keeps the question, command,
  and option labels out of the push entirely; the buttons stay.

### Changed
- The phone and web app's source now lives in `app/` of this repo.
- The default theme is now `terminal`, which uses your terminal's colors.
  Set `[theme] name = "catppuccin"` to keep the old look.

### Fixed
- The TUI client stays attached across `flockr server live-handoff`. It
  used to exit unless the build had the dev-only `dev-reload` feature.
- `flockr server live-handoff` with no `--import-exe` works after a rebuild
  replaced the running binary. It used to fail with
  "failed to spawn handoff import server at .../flockr (deleted)".
- An agent that finished in the visible tab counted as read even with no
  one looking (no client attached, or the terminal window not focused), so
  it never showed done and sent no alert. It now stays done until a
  focused client shows it or someone opens it.
- A live handoff turned agents that had finished but were not yet read
  back into idle. They now stay done.
- Server listeners (phone gateway, flock, web) bind as soon as the old
  server lets go during a live handoff, instead of after up to 10 seconds.
- The sample config showed the pane swap keys as unset and
  `pane_focus_passthrough` as `["nvim"]`; it now shows the real defaults.
  `--help` no longer lists `update` and `channel`, since flockr does not
  self-update.

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
