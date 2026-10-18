# flockr

A terminal workspace for running many coding agents side by side: workspaces,
tabs, split panes, an agent sidebar that shows which agents are working, done,
or waiting on you, and a socket API and plugins for automation.

flockr is a personal fork of [herdr](https://github.com/herdrdev/herdr) by
Can Celik and contributors, licensed under Apache-2.0. Most of the code is
theirs. Files in this repository have been modified from upstream: the code is
renamed so flockr installs and runs beside a stock herdr, and it carries the
changes below. For herdr itself, its docs, and support, go to
[herdr.dev](https://herdr.dev).

## Differences from herdr

Runs beside herdr:

- Separate binary, config dir (`~/.config/flockr`), socket, `FLOCKR_*` env
  vars, and agent hook files, so flockr and herdr don't see each other.
- Plugins and configs written for herdr work unchanged.
- No self-update. Update by pulling and rebuilding.

Sidebar:

- A title row with the menu. Agents sit under one header per space, and a
  worktree's agents nest under their repo the way the spaces list shows them.
- All and Active tabs filter the agents list.
- Agent rows can show the agent's own branch (`branch`), following it into
  the worktree it works in. Space rows show uncommitted changes (`diff_stat`),
  worktree spaces included.
- Agents and spaces are numbered, with `alt+1..9` and `alt+shift+1..9` to
  jump to them.
- `ui.sidebar_position = "right"` moves the sidebar to the right edge.

Agents:

- `ui.seen_delay_ms` and `ui.seen_scope`: a done agent keeps its done badge
  until its pane has been on screen long enough, so stepping through agents
  doesn't clear the ones you pass. `prefix+a` jumps to the next agent that
  needs you, `prefix+u` marks one done again.
- `ui.keep_agents_in_use_first`: a pane whose agent starts a fresh session
  moves behind the agents in use, and a dotted ring marks fresh sessions.
- Resumed agents keep the CLI options they were started with. Codex runs
  without its shared daemon, so its hooks see the pane.

Keys and layout:

- A settings screen (`prefix+s`) in sections; changes save to the config
  file and apply at once.
- Command palette (`prefix+:` or `prefix+/`) and vim jump keys in lists.
- Link hints (`prefix+f`) to open or copy a visible link.
- tmux layout presets (`prefix+alt+1..8`, `prefix+space`) and tmux's split
  and resize keys.
- Custom command popups are titled with their description.

## Build

Needs Rust (via [rustup](https://rustup.rs)) and Zig 0.16.0. With
[mise](https://mise.jdx.dev), `mise install` in the repo sets up Zig.

```bash
mise exec -- cargo build --release --locked
ln -sf "$PWD/target/release/flockr" ~/.local/bin/flockr
```

Config lives at `~/.config/flockr/config.toml`. `flockr --default-config`
prints every option. Install agent state hooks with
`flockr integration install claude` (or another agent name).

## License

Apache-2.0, same as upstream. See [LICENSE](LICENSE).
