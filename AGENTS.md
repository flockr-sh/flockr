# flockr

Terminal workspace for coding agents. A fork of [herdr](https://github.com/herdrdev/herdr)
(Apache-2.0) with fixes and features upstream won't take. Runs beside a stock
herdr: own binary, `~/.config/flockr` (`flockr-dev` for debug builds), own
socket, `FLOCKR_*` env vars, own integration hooks.

## Upstream

- `origin` is `dmkenney/flockr`, `upstream` is `herdrdev/herdr`. Work on `master`.
- Code is renamed herdr → flockr by `scripts/flockr-rebrand`. It is idempotent.
  It also deletes upstream-only files (release, website, docs snapshots,
  contributor policy, CI, nix) listed in its `prune` array.
- Sync: `git fetch upstream && git merge upstream/master`, resolve conflicts,
  run `scripts/flockr-rebrand`, build, test, commit. `just install-hooks` sets
  the `merge=ours` driver that keeps flockr's own README, AGENTS.md, and
  justfile on merges (`.gitattributes`).
- The rebrand rewrites every "herdr" in `src/`, `tests/`, `skills/`,
  `docs/next/api/`, and `.agents/`. When a comment or string must name upstream,
  write "upstream", or add a protected pattern to the script.
- Self-update and background auto-update are off (`src/update.rs`,
  `src/app/mod.rs`): the upstream feed would install herdr over flockr.

## Toolchain

- Rust pinned by `rust-toolchain.toml`. Zig 0.16.0 for the vendored
  libghostty-vt build, pinned in `mise.toml`; run cargo through
  `mise exec --` or a shell with mise activated.
- `just test` and `just ci` use `cargo nextest`. Plain `cargo test` runs tests
  as threads in one process and some tests that touch env vars or the working
  directory fail there; that is not a regression.
- `bun` runs the integration asset tests.

```bash
just lint        # cargo fmt --check, clippy -D warnings
just test        # nextest + maintenance + hot-path + integration asset tests
just check       # lint + test + Windows target lint
just test-one <filter>
just build       # release binary at target/release/flockr
```

Run `just check` (or at least `just lint` and `just test`) before calling work
done. Windows cross-lint needs `xwin` and `just setup-windows-cross` once.

Test a dev build from inside a running flockr or herdr without touching it:
debug builds already use `~/.config/flockr-dev`. Clear inherited overrides:

```bash
env -u FLOCKR_SOCKET_PATH -u FLOCKR_CLIENT_SOCKET_PATH cargo run -- <command>
```

For TUI behavior, run the dev build in a detached `tmux -L <name>` session with
`XDG_CONFIG_HOME`/`XDG_STATE_HOME` pointed at a short temp dir (Unix socket
paths are limited to ~100 bytes), drive it with `tmux send-keys`, and read it
with `tmux capture-pane -p` (`-e` for colors). Kill the tmux server after.

## Principles

- **State is separate from runtime.** `AppState` is pure data, testable without
  PTYs or async. `PaneState` is separate from `PaneRuntime`.
- **Render is pure.** `compute_view()` handles geometry and mutations.
  `render()` takes `&AppState` and only draws.
- **No god objects.** `app/` is split into state, actions, and input. Keep it so.
- **Platform code is isolated** in `src/platform/<os>.rs`; shared traits and
  contracts in `src/platform/mod.rs`. Core modules have no `#[cfg(target_os)]`.
- **Detection is decoupled.** The detector reads a screen snapshot and never
  touches parser or viewport state.
- **Reuse UI patterns.** Mouse-first TUI. New dialogs, settings, and flows reuse
  existing modal structure, affordances, and close actions.

## Hot paths

Work reachable from view computation, rendering, background-pane resizing, PTY
parsing, detection, and client frame fanout is multiplicative: per byte, event,
or render × panes/tabs/workspaces × attached clients.

- In pane-scaled loops, use narrow terminal-state accessors. No snapshot
  formatting, process-tree inspection, filesystem I/O, or allocation when one
  scalar is enough.
- Keep terminal-core locks short. Keep hidden-pane and retained-render early
  exits.
- When widening work in these loops, profile 1 and 15+ panes with
  `just bench-render-scale` and report the delta.
- Prefer deterministic tests to wall-clock limits.

## Server/client boundary

The server owns runtime state; the TUI is one client.

- Shared runtime facts (pane/agent metadata, process and terminal state,
  events) go in server state and the JSON API.
- Presentation state (sidebar layout, tokens, colors, selection, modals, mouse,
  viewport, done-badge acknowledgement) stays in the client
  (`src/client/shell/`).
- Use neutral API names, not UI names like sidebar, row, card, or widget.

## Endpoint contract

Client and server talk over a versioned endpoint protocol.

- Generation-1 codecs are frozen: no added, removed, reordered, or
  reinterpreted fields or enum variants. Add a new codec or method instead.
- New JSON fields are optional or defaulted; new enums need an `Unknown`
  fallback. Treat enums reachable from a frozen codec as append-closed.
- `tests/fixtures/endpoint-method-shapes-v1.json` and the frozen digests guard
  this. Don't re-bless them for a wire change. Exception: the rebrand changes
  two schema descriptions, and the script rewrites those two digests.
- Bump `src/protocol/wire.rs::PROTOCOL_VERSION` only for an incompatible wire
  change, once per release.

## Where things live

- `src/main.rs`, `src/cli/`: CLI entry and subcommands. `--default-config`
  prints the documented sample config (the only config reference now).
- `src/config/model.rs`: config structs and defaults.
- `src/server/`: headless server, socket API, client commands.
- `src/app/`: `AppState`, actions, agents, API handlers.
- `src/client/shell/`: TUI client: sidebar (`agent_sidebar.rs`), input, agent
  done/seen projection (`endpoint_agent_state.rs`).
- `src/detect/manifests/`: bundled agent-detection rules.
- `src/integration/`: agent hook installers and assets.
- `crates/ghostty-vt/`, `vendor/libghostty-vt/`: terminal emulation core.

## Agent detection

- Base rules on evidence: `flockr agent read <pane> --source detection --format text`
  (`--format ansi` when styling matters) and `flockr agent explain <pane> --json`.
  Encode invariant controls as explicit AND/OR gates. Never match whole-pane
  incidental text or the user viewport.
- Iterate with the `flockr-throwaway-repro` skill: edit
  `src/detect/manifests/<agent>.toml`, copy it to
  `~/.config/flockr-dev/agent-detection/<agent>.toml`, run
  `flockr server reload-agent-manifests`. Restore or remove the override after.
- Unit-test the detection engine with synthetic manifests. Don't freeze a
  specific agent's screens or rule IDs in tests; validate those live.

## Vendored libghostty-vt

`vendor/libghostty-vt.vendor.json` records the vendored commit. Local patches
live in `vendor/patches/libghostty-vt/`, each indexed in
`vendor/libghostty-vt.patches.md` with reason, verification, and removal
condition. Recheck every patch when updating. `just test` verifies they apply.

## Code conventions

- No `unwrap()` in production code. Use `tracing`. `#[allow]` only with a
  comment saying why.
- Compile-gate platform code (`#[cfg(windows)]`, `#[cfg(unix)]`). `cfg!(...)`
  only for constants whose branches compile everywhere.
- No new dependencies without a reason.
- Integration asset versions (`FLOCKR_INTEGRATION_VERSION` and the matching
  constants) bump once per release, not per commit.
- New config keys: add to `src/config/model.rs` with a default and a parse
  test, and document them in the sample config in `src/main.rs`.
- Commit subjects: imperative plain sentence, no prefix, no trailing period.

## Local notes

Put PRDs, plans, and scratch notes under `.local/` (gitignored).
