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

- `ui.seen_delay_ms`: a done agent keeps its done badge until its pane has been
  on screen this long, so stepping through agents doesn't clear the ones you
  pass.
- Separate binary, config dir (`~/.config/flockr`), socket, `FLOCKR_*` env
  vars, and agent hook files, so flockr and herdr don't see each other.
- No self-update. Update by pulling and rebuilding.

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
