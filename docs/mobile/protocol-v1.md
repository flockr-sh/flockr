# flockr mobile protocol v1 (`mobile.v1`)

Status: draft contract for the prototype. Both the Rust server (`flockr`)
and the TypeScript app (`flockr-mobile`) implement this file independently.
When code and this file disagree, fix one of them; do not guess.

Key words: MUST, MUST NOT, SHOULD, MAY as in RFC 2119.

## 1. Scope and shape

- An opt-in **mobile gateway** inside the flockr server: a WebSocket listener,
  off by default, bound only to configured IP addresses.
- The phone runs no terminal emulator and no SSH. The server renders pane
  cells; the phone draws them.
- Everything on the socket is JSON text frames. The existing bincode
  endpoint codecs (`src/protocol/endpoint.rs`, `surface_delta.rs`,
  `surface_scroll.rs`, `wire.rs`) are **not** changed and **not** exposed.
  `PROTOCOL_VERSION` is not bumped. No new `ServerMessage`/`ClientMessage`
  variants. No new JSON API methods (the method-shape fixture stays as is).
- Four services over one connection:
  1. JSON API passthrough (same method names and payloads as the local API
     socket, filtered by an allowlist).
  2. Agent list stream (`agents.watch`).
  3. Terminal surface stream (subscribe to one pane; full frame, then deltas).
  4. Input and push-token registration.
- No TLS in v1. See section 12.

## 2. Config

New `[mobile]` section in `src/config/model.rs` (`MobileConfig`,
`MobilePushConfig`), documented in the sample config in `src/main.rs`, with a
parse test. All keys need a server restart. No settings-screen rows (rarely
changed).

| Key | Type | Default | Meaning |
|---|---|---|---|
| `mobile.enabled` | bool | `false` | Start the gateway. Nothing listens when false. |
| `mobile.listen` | array of strings | `[]` | IP addresses to bind, e.g. `["100.92.14.3"]`. Each entry is a literal IPv4/IPv6 address, optionally with `:port` (`[v6]:port` for IPv6). Unspecified addresses (`0.0.0.0`, `::`) are rejected with a config diagnostic. Hostnames are rejected. Empty list with `enabled = true` is a diagnostic and nothing listens. |
| `mobile.port` | u16 | `47390` | Port for `listen` entries that have none. |
| `mobile.advertise` | array of strings | `[]` | Hosts (IPs or DNS names) put in the pairing QR, in try order. Empty means "the `listen` addresses". Lets Dylan add a NetBird DNS name. |
| `mobile.name` | string | host name | Display name of this server in the app and in pushes. Default from `crate::platform::hostname()`, falling back to `"flockr"`. |
| `mobile.max_devices` | u32 | `8` | Paired device limit. Pairing fails with `too_many_devices` beyond it. |
| `mobile.pairing_ttl_secs` | u64 | `600` | Lifetime of a bootstrap code. Clamped to 60..=3600. |
| `mobile.push.enabled` | bool | `true` | Send Expo pushes (only matters when the gateway is enabled). |
| `mobile.push.blocked` | bool | `true` | Push when an agent becomes Blocked. |
| `mobile.push.done` | bool | `true` | Push when an agent becomes Done. |
| `mobile.push.confirm_delay_ms` | u64 | `5000` | Re-check delay before sending. Clamped to 0..=60000. |
| `mobile.push.expo_access_token` | string? | unset | Optional Expo "enhanced push security" token, sent as `Authorization: Bearer`. |

Sample config text (append to `src/main.rs` sample):

```toml
# Mobile gateway for the flockr phone app. Off by default.
# Binds only to the listed IP addresses (for example a NetBird or WireGuard
# address). v1 has no TLS: only list addresses on an encrypted private network.
# Requires a server restart.
# [mobile]
# enabled = false
# listen = []            # e.g. ["100.92.14.3"]
# port = 47390
# advertise = []         # hosts shown in the pairing QR; default: listen
# name = ""              # default: host name
# max_devices = 8
# pairing_ttl_secs = 600
# [mobile.push]
# enabled = true
# blocked = true
# done = true
# confirm_delay_ms = 5000
# expo_access_token = ""
```

Binding: the gateway binds each `listen` address independently. If a bind
fails (for example NetBird is not up yet at boot), it logs a `warn!` and
retries that address every 10 s until the server stops. It does not fall back
to other addresses.

## 3. Files (state dir)

All under `crate::config::state_dir()/mobile/` (so debug builds use
`flockr-dev`). Unix mode `0600` for files, `0700` for the directory. Writes are
atomic (write temp file in the same dir, `fsync`, rename). Share one helper;
`src/detect/manifest_update.rs::atomic_write` is the model.

| File | Writer | Content |
|---|---|---|
| `server-id` | server or CLI, first use | 32 random bytes, hex. Never leaves the host. |
| `pairing.json` | CLI writes, server consumes | The one pending bootstrap code, or absent. |
| `devices.json` | server (pair, push token, last seen), CLI (revoke) | Paired devices. |
| `gateway.json` | server | Bound addresses, for CLI status/warnings. Removed on clean shutdown. |

`server_id` (public fingerprint) = base64url-no-pad of the first 16 bytes of
`sha256(server-id file bytes)`. 22 characters.

`pairing.json`:

```json
{
  "version": 1,
  "code_sha256": "9f2c...64 hex",
  "created_at": 1761330000,
  "expires_at": 1761330600,
  "failed_attempts": 0,
  "consumed_by": null
}
```

`devices.json`:

```json
{
  "version": 1,
  "devices": [
    {
      "device_id": "d_q8Zt3mVfR1xKp0aB",
      "name": "Dylan's iPhone",
      "platform": "ios",
      "token_sha256": "4b1e...64 hex",
      "created_at": 1761330100,
      "last_seen_at": 1761334000,
      "push": {
        "expo_token": "ExponentPushToken[xxxxxxxxxxxxxxxxxxxxxx]",
        "registered_at": 1761330105
      }
    }
  ]
}
```

`gateway.json`: `{"version":1,"pid":1234,"bound":["100.92.14.3:47390"],"started_at":1761330000}`.

Unknown fields in these files are ignored and preserved on rewrite where
practical. A missing or unparsable `devices.json` means "no devices"; log a
`warn!` and do not overwrite an unparsable file (rename it to
`devices.json.bad-<unix>` first).

CLI and server can both write `devices.json`. Each writer re-reads, modifies,
and renames. A lost update needs a revoke and a pairing within the same
milliseconds; accepted for v1.

## 4. Secrets and tokens

Randomness: `getrandom` (already in `Cargo.lock` as 0.3.4; add as a direct
dependency). Encoding: base64url without padding (`base64` crate,
`URL_SAFE_NO_PAD`).

| Secret | Format | Lifetime | Stored as |
|---|---|---|---|
| Bootstrap code | 16 random bytes → 22 chars | single use, `pairing_ttl_secs`, max 5 failed attempts | `sha256` hex in `pairing.json` |
| Device token | `"fmd1_"` + 32 random bytes → 5 + 43 chars | until revoked | `sha256` hex in `devices.json` |
| Device id | `"d_"` + 12 random bytes → 2 + 16 chars | until revoked | plain |

Hashing: `sha256` over the full token string's UTF-8 bytes (prefix
included), lowercase hex. Comparison: compute the presented token's digest,
then compare the 32 digest bytes in constant time (XOR-fold all bytes, check
for zero; no early exit). No new crate for this.

Device token lifecycle:

1. Issued once in the `welcome` that completes pairing. Never shown again.
2. App stores it in `expo-secure-store` (Keychain on iOS).
3. Presented in every `hello`.
4. Ends only on revoke (CLI) or when the app sends `device.forget` (section
   9.9). No rotation in v1.

## 5. CLI

New `src/cli/mobile.rs`, dispatched as `"mobile"` in `src/cli.rs`. These
commands work on the state files directly and do not need the server to be
running.

```
flockr mobile pair [--ttl <secs>] [--no-wait] [--json]
flockr mobile devices [--json]                # same as `devices list`
flockr mobile devices list [--json]
flockr mobile devices revoke <device_id> | --all
flockr mobile status [--json]
```

`flockr mobile pair`:

1. Load config. If `mobile.enabled` is false or `listen` is empty, print a
   warning saying pairing will not complete until the gateway runs, and
   continue.
2. If `gateway.json` is absent or its pid is not alive, print a warning that
   the server gateway is not listening.
3. Ensure `server-id` exists. Generate a bootstrap code. Write
   `pairing.json` (replacing any pending code).
4. Print the QR (section 6) using Unicode half blocks
   (`qrcode::render::unicode::Dense1x2`), then the pairing URL on its own
   line for copy/paste, then `Expires in 10:00`.
5. Unless `--no-wait`: poll `pairing.json` every 500 ms. On
   `consumed_by` set, print `Paired <name> (<device_id>)` and exit 0. On
   expiry, delete `pairing.json`, print `Pairing code expired`, exit 1. On
   Ctrl-C, delete `pairing.json` (only if it still holds this code) and exit
   130.
6. `--json` prints `{"url":"flockr://pair?p=...","expires_at":...}` and
   implies `--no-wait`.

`devices list` prints a table: id, name, platform, paired date, last seen,
push (yes/no). `--json` prints the `devices` array without `token_sha256`.

`devices revoke <id>` removes the record and prints `Revoked <name>`. Unknown
id: exit 1. `--all` removes every record. The running gateway drops the
device's connections within 2 s (section 8.5).

`status` prints enabled, configured listen addresses, bound addresses (from
`gateway.json`), device count, pending pairing (expiry), push enabled.

## 6. Pairing payload (QR)

The QR encodes one URL:

```
flockr://pair?p=<base64url-no-pad(UTF-8 JSON)>
```

JSON (keys exactly as shown; the app MUST ignore unknown keys):

```json
{
  "v": 1,
  "name": "dk-desktop",
  "hosts": ["100.92.14.3", "dk-desktop.netbird.cloud"],
  "port": 47390,
  "sid": "Xb3k9QmT0aLz7Rwe2YvN1c",
  "code": "q1W2e3R4t5Y6u7I8o9P0aA",
  "exp": 1761330600
}
```

- `hosts`: `mobile.advertise`, else the host part of each `listen` entry.
  IPv6 literals without brackets.
- `port`: `mobile.port`. If listen entries use different ports, each host
  MAY be given as `"host:port"` (`"[v6]:port"`); the app uses an explicit
  port over `port`.
- `sid`: `server_id` from section 3.
- `exp`: unix seconds.

The app also accepts the same URL pasted as text. QR error-correction level
M. The `flockr://` scheme is registered by the app so the URL also opens it
from a link.

## 7. Transport

- URL: `ws://<host>:<port>/mobile/v1`. Any other path: HTTP 404 and close.
- WebSocket subprotocol: client offers `Sec-WebSocket-Protocol:
  flockr.mobile.v1`. Server MUST select it; if absent, reject the upgrade
  with HTTP 400.
- No HTTP endpoints besides the upgrade. Upgrade request headers capped at
  8 KiB.
- `Origin` is not checked: auth is an in-band token, never a cookie, so a
  browser page cannot ride an existing session.
- Text frames only. Binary frames: close `4000`.
- Each text frame is exactly one JSON object with a string field `t`
  (message type).
- WebSocket ping: server sends a WS ping every 20 s; no pong for 60 s closes
  the connection. The app also MAY send app-level `ping` (section 9.10).

Size limits:

| Direction | Message | Max |
|---|---|---|
| client → server | `input.paste`, `input.submit`, `input.text` | 1 MiB (matches `MAX_INPUT_PAYLOAD`) |
| client → server | anything else | 64 KiB |
| server → client | any message | 4 MiB |

Over the inbound cap: close `1009`. A server message that would exceed 4 MiB
is not sent: for surfaces the subscription ends with `frame_too_large`; for
API responses the server sends `api.response` with error code
`payload_too_large`.

### Close codes

| Code | Name | When |
|---|---|---|
| 1000 | normal | Clean close either side. |
| 1009 | too_big | Inbound frame over cap. |
| 4000 | protocol_error | Malformed JSON, missing `t`, binary frame, message before `hello`. |
| 4001 | unauthorized | Bad device token or unknown device. |
| 4002 | unsupported_version | No common version. |
| 4003 | revoked | Device revoked while connected. |
| 4004 | too_many_connections | Connection limits (section 12). |
| 4005 | pairing_failed | Bad, expired, or used bootstrap code. |
| 4008 | hello_timeout | No `hello` within 10 s. |
| 4009 | server_shutdown | Server stopping or live handoff. |
| 4029 | rate_limited | Auth failure rate limit hit. |

Before a 4xxx close the server SHOULD send an `error` message (section 9.1)
with the same `code` name so the app can show text. The app treats 4001,
4003, and 4005 as terminal (do not auto-reconnect; 4001/4003 delete the
stored token and show "re-pair"). Others: reconnect with backoff 0.5 s, 1 s,
2 s, 4 s, ... capped at 30 s, with jitter.

## 8. Handshake and auth

### 8.1 Message order

1. Client opens the socket and sends `hello` within 10 s.
2. Server answers `welcome` (success) or `error` then closes.
3. After `welcome`, either side may send any message.

Any message other than `hello` before `welcome`: close `4000`.

### 8.2 `hello` (client → server)

Reconnect with a stored token:

```json
{
  "t": "hello",
  "versions": [1],
  "app": { "version": "0.1.0", "platform": "ios", "build": "12" },
  "auth": { "device_id": "d_q8Zt3mVfR1xKp0aB", "token": "fmd1_..." },
  "max_in_flight": 2
}
```

First connect from a QR:

```json
{
  "t": "hello",
  "versions": [1],
  "app": { "version": "0.1.0", "platform": "ios" },
  "pair": { "code": "q1W2e3R4t5Y6u7I8o9P0aA", "device_name": "Dylan's iPhone", "sid": "Xb3k9QmT0aLz7Rwe2YvN1c" }
}
```

- Exactly one of `auth` or `pair`. Both or neither: close `4000`.
- `platform`: `"ios" | "android" | "web"`; anything else is stored as given.
- `device_name`: trimmed, 1..=64 chars; longer is truncated.
- `pair.sid`: the `sid` from the QR. If it does not equal this server's
  `server_id`, fail with `pairing_failed` (the QR was for another host).
- `max_in_flight`: surface flow-control window (section 10.6), 1..=8,
  default 2.

### 8.3 Version negotiation

Server picks the highest version in `versions` it supports. v1 servers
support only `1`. None in common: `error` `unsupported_version` with
`"supported": [1]`, close `4002`.

Inside v1, optional behavior is announced in `welcome.features`. The app
MUST NOT depend on a feature the server did not list. v1 servers list:

- `"api"`, `"agents.watch"`, `"surface"`, `"surface.history"`,
  `"input"`, `"push.expo"` (only when push is enabled in config).
- `"surface.shift"` if the server can emit `shift` in deltas (section 10.5).
  The app MUST still handle `shift` whenever it is present.

### 8.4 Server checks

Pair path:

1. Rate limit (section 12). 
2. Read `pairing.json`. Fail `pairing_failed` if absent, `consumed_by` set,
   expired, `failed_attempts >= 5`, or `sid` mismatch.
3. Constant-time compare of `sha256(code)` with `code_sha256`. On mismatch
   increment `failed_attempts` (rewrite file) and fail.
4. Count devices; at `max_devices` fail `too_many_devices` (close `4005`).
5. Create the device record (new id + token), write `devices.json`, set
   `pairing.json.consumed_by = device_id` (the waiting CLI prints success and
   the code is now dead).
6. Send `welcome` including `device.token`.

Auth path:

1. Rate limit.
2. Load `devices.json` (cached; reload when mtime changes). Find
   `device_id`; hash the token; constant-time compare. Unknown device or
   mismatch: `error` `unauthorized`, close `4001`. Do not say which part was
   wrong.
3. Update `last_seen_at` in memory; persist at most once per 10 minutes per
   device and on disconnect.
4. Send `welcome` without `device.token`.

### 8.5 `welcome` (server → client)

```json
{
  "t": "welcome",
  "version": 1,
  "server": {
    "name": "dk-desktop",
    "server_id": "Xb3k9QmT0aLz7Rwe2YvN1c",
    "flockr_version": "1.2.0",
    "boot_id": "b-7f3a..."
  },
  "device": { "device_id": "d_q8Zt3mVfR1xKp0aB", "name": "Dylan's iPhone", "token": "fmd1_..." },
  "features": ["api", "agents.watch", "surface", "surface.history", "input", "push.expo"],
  "methods": ["agent.list", "agent.get", "..."],
  "limits": {
    "max_in_flight": 2,
    "max_subscriptions": 4,
    "max_message_bytes": 65536,
    "max_input_bytes": 1048576,
    "max_cols": 500,
    "max_rows": 200
  }
}
```

- `device.token` is present only on the pair path.
- `server.boot_id`: random per server process; the app can show "server
  restarted" when it changes.
- The app MUST check `server.server_id` equals the `sid` it paired with. On
  mismatch it closes and shows "this address is now a different flockr
  server". (Not cryptographic; see section 12.)
- `methods`: the API allowlist (section 9.2) as sent by this server.

### 8.6 Revocation

Every 2 s the gateway reloads `devices.json` if its mtime changed and then
checks every live connection against the device list (on every tick, not
only on a reload). If a connected device's record is gone, it sends `error`
`revoked` and closes `4003` at once, skipping any queued output, ending all its
subscriptions (fit-mode sizes are restored, section 10.2). Its push token
goes with the record, so no more pushes.

## 9. Messages

Conventions:

- `t`: message type, a string. Request-style messages carry a client-chosen
  string `id` (1..=64 chars, unique among the connection's open requests);
  the reply carries the same `id`.
- Unknown fields: ignore. Unknown enum string values: map to an `unknown`
  fallback, never fail the whole message.
- Unknown `t` from client: if it has an `id`, server replies
  `{"t":"error","id":...,"code":"unknown_type"}`; else it ignores it
  (`debug!` log). Unknown `t` from server: app ignores it.
- Integers are JSON numbers within 2^53.

### 9.1 `error` (server → client)

```json
{ "t": "error", "id": "r7", "code": "not_found", "message": "pane p_9 not found" }
```

`id` present when the error answers a request. Codes:

| Code | Meaning |
|---|---|
| `bad_request` | Message shape invalid. |
| `unknown_type` | Unknown `t` with an `id`. |
| `unauthorized` | Auth failed. |
| `pairing_failed` | Pair failed (any reason). |
| `too_many_devices` | Pair refused at `max_devices`. |
| `unsupported_version` | Version negotiation failed. |
| `revoked` | Device revoked. |
| `too_many_connections` | Connection limit. |
| `rate_limited` | Rate limit. |
| `method_not_allowed` | API method not in allowlist. |
| `not_found` | Target pane/terminal/agent/subscription not found. |
| `pane_busy` | Fit subscribe refused: terminal already has an attach owner. |
| `too_many_subscriptions` | Over `max_subscriptions`. |
| `payload_too_large` | Reply or frame would exceed caps. |
| `push_unavailable` | Push disabled in config. |
| `server_unavailable` | Server loop or API socket did not answer. |
| `internal` | Bug; see server log. |

### 9.2 API passthrough

Request:

```json
{ "t": "api.request", "id": "r1", "method": "agent.list", "params": {} }
```

`method` and `params` are exactly what the local API socket takes
(`src/api/schema.rs`), i.e. the server forms
`{"id": <id>, "method": <method>, "params": <params>}` and sends it as one
line. Response:

```json
{ "t": "api.response", "id": "r1", "response": { "id": "r1", "result": { "type": "agent_list", "agents": [] } } }
```

`response` is the API's own JSON line, parsed and embedded unchanged (success
`{"id","result"}` or error `{"id","error":{"code","message"}}`). The app
parses it with the same types as the CLI JSON output.

Streaming methods (`events.subscribe`): the first line comes as
`api.response`; every later line comes as:

```json
{ "t": "api.event", "id": "r2", "event": { "event": "pane.agent_status_changed", "data": { "pane_id": "p_3", "workspace_id": "w_1", "agent_status": "blocked" } } }
```

until the API closes the stream or the app sends:

```json
{ "t": "api.cancel", "id": "r2" }
```

which closes the underlying stream. The server then sends
`{"t":"api.end","id":"r2"}`. `api.end` is also sent when the API side closes
first.

Long-running methods (`events.wait`, `agent.wait`, `agent.prompt` with wait,
`pane.wait_for_output`) work the same as one request with one response; the
app MAY `api.cancel` them.

Allowlist (sent in `welcome.methods`; anything else answers
`method_not_allowed` without touching the API):

```
ping
session.snapshot
workspace.list workspace.get workspace.focus workspace.create workspace.rename workspace.close
tab.list tab.get tab.focus tab.create tab.rename tab.close
agent.list agent.get agent.read agent.explain agent.send_keys agent.rename agent.focus
agent.start agent.prompt agent.wait
pane.list pane.get pane.current pane.layout pane.read pane.focus pane.split pane.close pane.zoom
pane.send_text pane.send_keys pane.send_input pane.scroll pane.resize pane.rename
pane.mark_unread pane.link.list pane.process_info
worktree.list worktree.create worktree.open
events.subscribe events.wait pane.wait_for_output
notification.show
```

Not allowed: `server.*`, `plugin.*`, `integration.*`, `client.*`,
`client_shell.*`, `command.invoke`, `layout.*`, `pane.edit_scrollback`,
`pane.input.set`, `pane.selection.read`, `pane.copy_*`, `pane.link.activate`,
`pane.link.resolve`, `pane.report_*`, `pane.clear_agent_authority`,
`pane.release_agent`, `pane.move`, `pane.swap`, `popup.close`,
`agent.view.*`, `worktree.remove`, release notes and announcements. The list
is a guard against server lifecycle changes and code installs from the
phone, not a security boundary: `pane.send_text` into a shell already runs
any command as Dylan.

Note: `pane.scroll`, `pane.resize`, `pane.focus`, `pane.zoom` change shared
state; the desktop sees the change. That matches the local CLI.

### 9.3 Agent list stream

```json
{ "t": "agents.watch", "id": "w1" }
```

Server replies at once, and again after every relevant change, with:

```json
{
  "t": "agents",
  "id": "w1",
  "rev": 14,
  "agents": [ { "terminal_id": "t_5", "pane_id": "p_3", "workspace_id": "w_1", "tab_id": "tab_2", "agent": "claude", "display_agent": "Claude", "agent_status": "blocked", "state_change_seq": 41, "revision": 88, "focused": false } ],
  "workspaces": [ { "workspace_id": "w_1", "number": 1, "label": "flockr", "focused": true, "agent_status": "blocked", "active_tab_id": "tab_2", "pane_count": 3, "tab_count": 2 } ],
  "tabs": [ { "tab_id": "tab_2", "workspace_id": "w_1", "number": 2, "label": "mobile", "focused": true, "pane_count": 2, "agent_status": "blocked" } ]
}
```

- `agents`, `workspaces`, `tabs` are the unmodified arrays from the API
  results of `agent.list`, `workspace.list`, `tab.list` (`AgentInfo`,
  `WorkspaceInfo`, `TabInfo` in `src/api/schema/`). Every field those types
  have is passed through; the example is trimmed.
- `agent_status`: `"idle" | "working" | "blocked" | "done" | "unknown"`.
  Unknown future values map to `unknown`.
- `rev`: increments per message on this watch.
- Sorting is the app's job (Blocked, Done, Working, Idle, Unknown; then
  workspace number, tab number).
- `{"t":"agents.unwatch","id":"w1"}` stops it; server sends
  `{"t":"api.end","id":"w1"}`. One watch per connection; a second
  `agents.watch` replaces the first.

Server behavior: poll `EventHub::events_after(seq)` every 250 ms. If any
event arrived (any kind), re-run the three list calls and send `agents` if
the combined result differs from the last one sent. At most one refresh per
250 ms.

### 9.4 Surface messages (client → server)

See section 10 for the frame format and server behavior.

```json
{ "t": "surface.subscribe", "id": "s1", "sub": 1, "target": "p_3", "mode": "observe", "max_fps": 30 }
{ "t": "surface.subscribe", "id": "s2", "sub": 2, "target": "p_3", "mode": "fit", "cols": 46, "rows": 38 }
{ "t": "surface.resize", "sub": 2, "cols": 80, "rows": 30 }
{ "t": "surface.ack", "sub": 1, "seq": 17 }
{ "t": "surface.resync", "sub": 1 }
{ "t": "surface.scroll", "sub": 1, "lines": 10 }
{ "t": "surface.scroll", "sub": 1, "to": "bottom" }
{ "t": "surface.history", "id": "h1", "sub": 1, "lines": 500 }
{ "t": "surface.unsubscribe", "sub": 1 }
```

- `sub`: client-chosen integer 1..=2^31, unique among the connection's open
  subscriptions.
- `target`: pane id, terminal id, or agent target; resolved the same way as
  `flockr terminal observe <target>` (`resolve_terminal_target_id_string`).
- `mode`: `"observe"` (default) or `"fit"` (section 10.2).
- `cols`/`rows`: required for `fit`, ignored for `observe`. Clamped to
  2..=`max_cols`, 2..=`max_rows`.
- `max_fps`: 1..=60, default 30. Server sends at most this many
  frames/deltas per second for the subscription.
- `surface.resize`: fit mode only; observe answers `bad_request`.
- `surface.scroll`: `lines > 0` scrolls up into history, `< 0` down;
  `to: "bottom"` resets. Moves the terminal's shared viewport (same as
  `pane.scroll`; the desktop sees it). Takes effect on the next frame.
  `lines` is relative to the last offset this connection asked for (within
  the last second), so several scrolls between frames add up.
- `surface.history`: plain-text history (section 10.7).

### 9.5 Surface messages (server → client)

```json
{ "t": "surface.started", "id": "s1", "sub": 1, "pane_id": "p_3", "terminal_id": "t_5", "mode": "observe", "cols": 120, "rows": 40, "theme": { "fg": "#d8dee9", "bg": "#1e1e2e", "palette": ["#000000", "#cc0000", null, "..."] } }
{ "t": "surface.frame", "sub": 1, "seq": 1, "...": "section 10.4" }
{ "t": "surface.delta", "sub": 1, "seq": 2, "base": 1, "...": "section 10.5" }
{ "t": "surface.history.result", "id": "h1", "sub": 1, "lines": ["$ cargo test", "..."] }
{ "t": "surface.ended", "sub": 1, "reason": "pane_closed" }
```

- `surface.started` answers `surface.subscribe` (same `id`). Errors answer
  with `error` and that `id`.
- `theme`: optional. `fg`, `bg`: `"#rrggbb"` or `null`. `palette`: 16
  entries (`"#rrggbb"` or `null`). `null` or missing means "use the app's
  built-in xterm default". Source: the server's effective host terminal
  theme (`crate::terminal_theme::TerminalTheme`) if any client has reported
  one, else omitted.
- `surface.ended.reason`: `"unsubscribed" | "pane_closed" | "revoked" |
  "frame_too_large" | "taken_over" | "server_shutdown" | "error"`; unknown →
  treat as `"error"`. No more messages for that `sub` after it.

### 9.6 Input (client → server)

Exactly one of `sub` (an open subscription's terminal) or `target` (resolved
like `surface.subscribe`).

```json
{ "t": "input.text",   "id": "i1", "sub": 1, "text": "y" }
{ "t": "input.key",    "id": "i2", "sub": 1, "keys": ["ctrl+c"] }
{ "t": "input.paste",  "id": "i3", "sub": 1, "text": "multi\nline" }
{ "t": "input.submit", "id": "i4", "sub": 1, "text": "fix the failing test" }
```

`id` is optional; when present the server replies
`{"t":"input.ok","id":"i1"}` or `error`. Without `id`, failures are logged
only.

| Type | Meaning | Server path |
|---|---|---|
| `input.text` | Typed text, no paste brackets (each soft-keyboard commit). | `ClientPaneInputEvent::TextCommit` through `apply_client_pane_input_events` (`src/server/pane_input.rs`). |
| `input.key` | Named keys, in order. | `pane.send_keys` (same key grammar as `flockr pane send-keys`, encoded by `encode_api_keys` so kitty keyboard mode is honored). |
| `input.paste` | Paste; wrapped in paste brackets when the program in the pane enabled bracketed paste. | `pane.send_text`. |
| `input.submit` | Composer "send": text as paste, then Enter. | `pane.send_input` with `keys: ["Enter"]` set explicitly (the JSON default for `keys` is empty; only the CLI defaults to Enter). |

Key names (`config::parse_key_combo` grammar, lower case): `enter`, `esc`,
`tab`, `shift+tab`, `backspace`, `delete`, `insert`, `up`, `down`, `left`,
`right`, `home`, `end`, `pageup`, `pagedown`, `f1`..`f12`, `space`, single
characters, and modifiers `ctrl+`, `alt+`, `shift+` (e.g. `ctrl+c`,
`ctrl+d`, `alt+b`, `ctrl+left`). The app's control-key row sends these. Bad
key: `error` `bad_request` naming the key (API code `invalid_key` is mapped
to `bad_request`).

Mouse input is not in v1.

### 9.7 Push token

```json
{ "t": "push.register", "id": "p1", "expo_token": "ExponentPushToken[xxxxxxxxxxxxxxxxxxxxxx]", "platform": "ios" }
{ "t": "push.unregister", "id": "p2" }
```

Reply: `{"t":"push.ok","id":"p1"}` or `error` (`push_unavailable` when
`mobile.push.enabled = false`; `bad_request` if the token does not match
`^(Exponent|Expo)PushToken\[[A-Za-z0-9_-]{1,256}\]$`). Stored in the device's
`push` field; one token per device (register replaces).

### 9.8 App state

```json
{ "t": "app.state", "foreground": true }
```

Sent on connect (after `welcome`) and on every AppState change. While a
device has at least one live connection with `foreground: true`, it gets no
Expo pushes; the server sends the alert in-band instead. A connection counts
as live only if the phone sent something (a pong answers every 20 s ping)
within the last 25 s, because a suspended app may never send `foreground:
false`:

```json
{ "t": "notify", "kind": "blocked", "pane_id": "p_3", "workspace_id": "w_1", "tab_id": "tab_2", "terminal_id": "t_5", "title": "Claude is blocked", "body": "flockr › mobile · dk-desktop" }
```

`kind`: `"blocked" | "done"`; unknown → ignore. Same trigger rules as push
(section 11).

### 9.9 Device forget

```json
{ "t": "device.forget", "id": "f1" }
```

The server deletes this device's record, replies `{"t":"device.ok","id":"f1"}`,
then closes `1000`. Used by the app's "Remove this server" action.

### 9.10 Ping

```json
{ "t": "ping", "ts": 1761330000123 }
{ "t": "pong", "ts": 1761330000123 }
```

Server echoes `ts` unchanged. The app uses it to show round-trip time in the
prototype's debug overlay.

### 9.11 `bye` (server → client)

```json
{ "t": "bye", "reason": "server_shutdown" }
```

Sent before the server closes for shutdown or live handoff (close `4009`).
The app reconnects with backoff.

## 10. Terminal surface stream

### 10.1 Server structure (reuse)

Each subscription is one headless `ClientConnection`, exactly like the
existing `flockr terminal observe` / `attach` clients:

- `observe` → `ClientConnectionMode::TerminalObserve { terminal_id }`.
- `fit` → `ClientConnectionMode::TerminalAttach { terminal_id }`, with the
  same owner, `direct_attach_resize_locks`, and geometry-restore logic as
  `attach_terminal_client` / `remove_client_and_resize_if_needed`
  (`src/server/headless.rs`).

That reuses the PTY render-target visibility rules (`pty_render_targets`),
synchronized-output deferral, and the per-client render early exits in
`src/server/headless/render.rs` with no new hot-path branches for non-mobile
clients.

What is new:

1. `ClientWriter::in_process()` in `src/server/client_transport.rs`, built
   like the existing `ClientWriter::test_channel`: same `ClientWriterQueue`
   (render lane capacity 1, latest wins), drained by a gateway thread instead
   of a socket writer.
2. `ClientRenderState::Cells { last: Option<FrameData> }` in
   `src/server/render_stream.rs`. `prepare_frame` skips when the frame equals
   `last`; otherwise it yields `ServerMessage::PaneSurface(PaneSurfaceFrame)`
   with `frame`, one `PaneSurfacePane` (scroll metrics,
   `alternate_screen_active`, `mouse_reporting`), empty splits, no popup, no
   graphics. This reuses an existing in-memory type and the existing framing;
   it is not a wire change because the bytes never leave the process.
3. In the `TerminalObserve` arm of `render.rs`, a mobile observe client uses
   `runtime.current_size()` (returns `(rows, cols)`) as the render area, so
   the phone always gets the whole grid at the pane's real size. One scalar
   read per mobile client per render.
4. New internal `ServerEvent` variants (not wire types):
   `MobileSurfaceOpen { client_id, target, fit, cols, rows, writer, reply }`
   and `MobilePaneInput { target, events: Vec<ClientPaneInputEvent>, reply }`,
   handled in `src/server/headless/mobile.rs`. Resize and close reuse the
   existing `ClientResize` and `ClientDisconnected` events, which already
   resize an attached terminal and restore the desktop geometry. A mobile
   client is recognized by its `ClientRenderState::Cells` render state; no new
   `ClientConnection` field.

Gateway side, per subscription: a blocking drain thread reads framed
`ServerMessage`s from the in-process queue with `protocol::read_message`,
keeps the newest `PaneSurface` in a `tokio::sync::watch` channel, and handles
`ServerShutdown` (→ `surface.ended`). The connection task diffs and encodes
JSON (section 10.4–10.5) off the server loop.

Cost: zero when no phone is subscribed. With a phone, one virtual render per
changed frame per subscription, the same as one `terminal observe` client.
Run `just bench-render-scale` before and after and report the delta (it
should be unchanged; it has no mobile clients).

### 10.2 Modes

- `observe`: no PTY resize. The phone gets the pane at its real `cols × rows`
  and scales or pans it. Size changes when the desktop layout changes; the
  server then sends a full frame with the new size. Many observers per pane
  are fine.
- `fit`: the PTY is resized to the phone's `cols × rows` (agent TUIs reflow
  to phone width). Like `flockr terminal attach`: the desktop layout for that
  pane is held at the phone's size until the subscription ends, then
  restored. If the terminal already has an attach owner, subscribe fails with
  `pane_busy`, except when the owner is a fit surface of the same paired
  device (an old connection after a network switch): then the new
  subscription takes it over and the old one ends `taken_over`. If a local
  `attach --takeover` takes it, the phone gets `surface.ended` `taken_over`.

The prototype SHOULD start in `observe` and offer a "fit to phone" toggle.

### 10.3 Cell model

The app keeps, per subscription: `cols`, `rows`, a style table, a link table,
`rows` lines of runs, cursor, scroll info.

**Colors** (integers):

| Value | Meaning |
|---|---|
| `-1` | Default (theme fg for fg, theme bg for bg). |
| `0..=255` | Palette index (0–15 from theme palette, 16–255 xterm cube/greys). |
| `>= 16777216` | Truecolor: `value - 16777216` is `0xRRGGBB`. |

Server mapping from `CellData.fg/bg` (`color_to_u32` in `wire.rs`): high
byte `0x00`: low byte `0` → `-1`; `1..=16` (ratatui named
Black..White) → `0..=15` in order Black, Red, Green, Yellow, Blue, Magenta,
Cyan, Gray, DarkGray, LightRed, LightGreen, LightYellow, LightBlue,
LightMagenta, LightCyan, White. High byte `0x01` → low byte as index. High
byte `0x02` → `16777216 + (value & 0xFFFFFF)`.

**Flags** (integer bit set):

| Bit | Meaning |
|---|---|
| 0 (1) | bold |
| 1 (2) | dim |
| 2 (4) | italic |
| 3 (8) | underline |
| 4 (16) | slow blink |
| 5 (32) | rapid blink |
| 6 (64) | reverse |
| 7 (128) | hidden |
| 8 (256) | strikethrough |
| 9–12 | underline style: 0/1 single, 2 double, 3 curly, 4 dotted, 5 dashed |

Bits 0–8 are `CellData.modifier & 0x1FF` (ratatui `Modifier` bits); bits 9–12
are `underline_style_from_modifier(modifier)`. The app applies reverse by
swapping resolved fg/bg. Hidden: draw background only. Blink: draw as normal
in v1.

**Style** = JSON array `[fg, bg, flags]` or `[fg, bg, flags, link]` where
`link` is an index into the link table (OSC 8 URI). Style id = index in the
style table. Style `0` is always `[-1, -1, 0]`.

**Run** = one of:

- `[x, s, text]` — starts at column `x`, style id `s`. Every UTF-16 code unit
  of `text` is one grapheme of width 1, so the run covers `text.length`
  columns and the character at index `i` sits at column `x + i`.
- `[x, s, text, w]` — exactly one grapheme `text` (any length in code units)
  covering `w` columns (`w` is 1 or 2).

The server emits the 4-element form for any grapheme that is not a single
BMP code point of display width 1: wide (CJK, most emoji), combining
sequences, surrogate pairs, ZWJ sequences. For a width-2 grapheme the server
skips the next cell (the wide-char continuation), whatever it holds. Width
comes from `unicode-width` on the cell symbol (already a dependency). Empty
symbols are treated as a space.

**Line** = array of runs, sorted by `x`, not overlapping. Columns covered by
no run are blank with style 0. The server MUST omit trailing default-style
spaces and SHOULD omit any default-style space-only gap by starting the next
run at its column. Adjacent cells with the same style and 1-column text join
into one run.

Example row: `"$ ls"` in default style, then `"main.rs"` bold green, a wide
`"漢"`, then red background spaces to column 20:

```json
[[0, 0, "$ ls"], [5, 3, "main.rs"], [13, 0, "漢", 2], [15, 4, "     "]]
```

with styles `3 = [2, -1, 1]`, `4 = [-1, 1, 0]`.

**Cursor**: `{"x": 4, "y": 23, "visible": true, "shape": 0}` or `null`.
`shape` is the DECSCUSR parameter (0 default, 1/2 block, 3/4 underline, 5/6
bar; odd = blinking). Hidden when the terminal is scrolled back (the server
already sets `visible` false then).

**Scroll**: `{"offset": 0, "max": 1834, "viewport": 40}` from
`PaneSurfaceScrollMetrics` (`offset_from_bottom`, `max_offset_from_bottom`,
`viewport_rows`). `null` when unknown.

### 10.4 Full frame

```json
{
  "t": "surface.frame",
  "sub": 1,
  "seq": 1,
  "cols": 120,
  "rows": 40,
  "styles": [[-1, -1, 0], [16777216, -1, 1], [2, -1, 1], [-1, 1, 0]],
  "links": ["https://github.com/flockr-sh/flockr"],
  "lines": [[[0, 0, "$ ls"]], [], [[0, 2, "ok"]]],
  "cursor": { "x": 2, "y": 39, "visible": true, "shape": 0 },
  "scroll": { "offset": 0, "max": 1834, "viewport": 40 },
  "alt": false
}
```

- Replaces everything: size, style table, link table, all lines.
- `lines.length == rows`.
- `links` MAY be omitted when empty.
- `alt`: alternate screen active (TUI app running). The app MAY use it to
  hide the history affordance.
- Sent first after `surface.started`, after `surface.resync`, on size
  change, when the style table would pass 4096 entries or the link table 256,
  and when a delta would replace every row without a shift (it would then be
  no smaller than the full frame).

### 10.5 Delta

```json
{
  "t": "surface.delta",
  "sub": 1,
  "seq": 2,
  "base": 1,
  "shift": 3,
  "styles_add": [[1, -1, 0]],
  "links_add": [],
  "lines": [[37, [[0, 0, "test result: ok"]]], [38, []], [39, [[0, 0, "$"]]]],
  "cursor": { "x": 2, "y": 39, "visible": true, "shape": 0 },
  "scroll": { "offset": 0, "max": 1837, "viewport": 40 },
  "alt": false
}
```

Apply in this order:

1. If `base` is not the app's current `seq`, drop the delta and send
   `surface.resync` (should not happen on an ordered socket; it is a bug
   guard).
2. `styles_add`: append to the style table (ids continue from the current
   length). `links_add`: same for links.
3. `shift` (optional, default 0): `shift = n > 0` moves the whole grid up by
   `n` rows: new line `y` = old line `y + n` for `y < rows - n`; the last `n`
   lines become empty. `n < 0` moves down symmetrically. `|n| < rows`.
4. `lines`: each `[y, runs]` replaces line `y` completely.
5. `cursor`, `scroll`, `alt`: replace when present; absent means unchanged.
6. Set current `seq` to `seq`.

`seq` increases by 1 per frame or delta per subscription, starting at 1.
Deltas never change `cols`/`rows`.

Server diff (in `src/server/mobile/cells.rs`): compare the new
`FrameData` with the last one sent, row by row (symbol, colors, modifiers,
and link URI; per-row hashes first, then an exact check). Shift detection
(when listing `surface.shift`): if at least half the rows already match
without a shift, send no shift. Otherwise try `n` in 1..=rows/2 in order and
accept the first `n` where at least half of the overlapping rows match
(`last[y+n] == next[y]`) and more rows match than with no shift; then send
only rows that still differ. The server only emits `n > 0`; the app still
handles `n < 0`. Encode rows from the new frame only. Deltas always carry
`cursor`, `scroll`, and `alt`.

### 10.6 Flow control

- The server keeps `in_flight = frames/deltas sent − highest acked seq` per
  subscription. While `in_flight >= max_in_flight` (from `hello`, default 2)
  it sends nothing for that subscription. On ack it diffs the newest frame
  against the last sent one and sends one message.
- The app MUST send `surface.ack {sub, seq}` after it has applied (and
  scheduled drawing of) each frame or delta. Acks are cumulative.
- `max_fps` caps sends: at most one message per `1000 / max_fps` ms per
  subscription; changes in between coalesce.
- Server-side, the in-process render lane already keeps only the newest
  frame, so a slow phone never queues stale frames on the server loop.
- If the WebSocket send buffer for the connection exceeds 8 MiB, the server
  closes `4000` with `error` `bad_request` "client too slow" (the app's acks
  should prevent this).

This lets the prototype measure drawing speed directly: the server frame
rate settles at what the phone can apply.

### 10.7 History

`surface.history` returns recent plain text for the subscription's pane:
`pane.read` with `source: "recent"`, `format: "text"`, `lines` (1..=10000,
default 500). Reply:

```json
{ "t": "surface.history.result", "id": "h1", "sub": 1, "lines": ["line 1", "line 2"] }
```

Oldest first. No styles in v1. Viewport scrolling with styles uses
`surface.scroll` (shared viewport, section 9.4).

### 10.8 Drawing notes for the app (Skia)

Not part of the wire contract; guidance for a fast first version.

- One monospace font (bundled, e.g. JetBrains Mono). Measure the cell width
  once (`font.measureText("M")` advance) and line height from metrics.
- Keep one `SkPicture` per line. On a delta, re-record only lines in
  `lines`; `shift` just reorders the picture array. Draw the grid as a list
  of pictures translated by `y * lineHeight`.
- Per run: draw the background rect first (skip when bg resolves to the
  theme bg), then the text. For 3-element runs draw the whole string at
  `x * cellWidth` (monospace advance matches cells). For 4-element runs draw
  the grapheme at `x * cellWidth`, constrained to `w * cellWidth`.
- Resolve styles to Paint objects once per style id and cache them.
- Apply frames on the JS thread, but keep the per-line pictures in a shared
  value / ref so React re-renders do not rebuild them.
- Show a debug overlay: frames/s applied, average apply+record ms, last
  `pong` RTT, bytes/s. This is the prototype's main output.

## 11. Push

### 11.1 Source and triggers

The push worker (`src/server/mobile/push.rs`) polls `EventHub` every 250 ms
for `pane.agent_status_changed` events (all panes; `EventData::
PaneAgentStatusChanged`).

For each event:

1. Let `status` = the event's `agent_status`. Track `last_status[pane_id]`.
2. If `status == last_status[pane_id]`, ignore. Update `last_status`.
3. If `status` is `blocked` (and `push.blocked`) or `done` (and
   `push.done`), schedule a check for this pane at now +
   `confirm_delay_ms`, replacing any pending check for the pane. Any other
   status cancels the pending check.

`done` here is the server's `AgentStatus::Done` (idle and not yet seen,
`pane_agent_status` in `src/app/api_helpers.rs`). If Dylan is looking at the
pane on the desktop it becomes `idle` instead and no push goes out.

### 11.2 Confirm, dedupe, send

At check time:

1. `agent.get` for the pane (in-process API call). If missing, or its
   `agent_status` differs from the scheduled status, drop.
2. Dedupe key: `(pane_id, status, state_change_seq)`. If already sent, drop.
   Keep the last 1024 keys.
3. Build the alert (11.3). For each paired device with a push token:
   - device has a live foreground connection (`app.state`, section 9.8) →
     send in-band `notify` on its connections instead;
   - else queue an Expo message.
4. Per-device cap: 30 pushes per minute; drop extra and `warn!`.

### 11.3 Content

- Agent name: `display_agent`, else `name`, else `agent`, else `"Agent"`.
- Workspace label from `workspace.list`, tab label from `tab.list`.
- Host: `mobile.name`.
- Title: `"<agent> is blocked"` or `"<agent> finished"`.
- Body: `"<workspace> › <tab> · <host>"`.

### 11.4 Expo request

`POST https://exp.host/--/api/v2/push/send`, headers `Content-Type:
application/json`, `Accept: application/json`, and `Authorization: Bearer
<expo_access_token>` when configured. Body: JSON array, at most 100
messages per request.

```json
[
  {
    "to": "ExponentPushToken[xxxxxxxxxxxxxxxxxxxxxx]",
    "title": "Claude is blocked",
    "body": "flockr › mobile · dk-desktop",
    "sound": "default",
    "priority": "high",
    "channelId": "agents",
    "interruptionLevel": "time-sensitive",
    "ttl": 3600,
    "data": {
      "type": "agent_status",
      "server_id": "Xb3k9QmT0aLz7Rwe2YvN1c",
      "server_name": "dk-desktop",
      "status": "blocked",
      "agent": "Claude",
      "workspace_id": "w_1",
      "workspace": "flockr",
      "tab_id": "tab_2",
      "tab": "mobile",
      "pane_id": "p_3",
      "terminal_id": "t_5"
    }
  }
]
```

- `interruptionLevel`: `"time-sensitive"` for blocked, `"active"` for done.
- App on tap: find the server by `data.server_id`, connect, open the pane.
  Android channel `agents` is created by the app at startup.

Response handling (`{"data":[ticket,...]}` in request order):

- `{"status":"ok"}`: done (receipts are not polled in v1).
- `{"status":"error","details":{"error":"DeviceNotRegistered"}}`: clear
  that device's `push` field in `devices.json`.
- Other ticket errors: `warn!` with the error string.
- HTTP 429 or 5xx or network error: retry the whole batch after 2 s, then
  8 s, then drop with `warn!`.
- Never log tokens in full (log the last 6 characters).

Sending runs on one dedicated thread fed by a bounded channel (capacity
256, drop oldest on overflow), never on the server loop or a tokio worker.

## 12. Security and limits

No TLS in v1, on purpose:

- The gateway binds only to listed specific IPs; `0.0.0.0`/`::` are
  rejected. Dylan lists his NetBird (WireGuard) address. WireGuard encrypts
  and authenticates peers at the network layer; adding TLS would need
  certificates the phone can trust, which a QR pin could cover in v2.
- Tokens and terminal content travel in clear inside the tunnel. Anyone who
  can read traffic on that interface (another process on a NetBird peer
  that can sniff, or a misconfigured listen address on a LAN) can see them.
  Document this in the sample config (done above) and in the app's pairing
  screen.
- `server_id` is a fingerprint check, not proof: it catches pairing with the
  wrong host or an address that moved, not an active impostor on the
  network. WireGuard peer keys cover that.
- Device tokens: 256-bit, stored as SHA-256 only, constant-time compare,
  revocable. Bootstrap codes: 128-bit, single use, short TTL, 5 failed
  attempts kill the code.

Limits (constants in `src/server/mobile/mod.rs`):

| Limit | Value |
|---|---|
| Paired devices | `mobile.max_devices` (8) |
| Connections per device | 4 |
| Connections total | 16 |
| Unauthenticated connections | 8 at once, 3 per remote IP; refused (socket dropped, no upgrade) when full; upgrade plus `hello` within 10 s, and a timeout counts as an auth failure |
| Outbound writes | one frame may take 30 s to write, then the connection closes |
| Auth/pair failures per remote IP | 5 per 60 s, then close `4029` for 60 s |
| Pair attempts total | 10 per 60 s |
| Subscriptions per connection | 4 |
| Open API requests/streams per connection | 32 |
| Inbound messages per connection | 200 per s (over: drop and `rate_limited` error once per s) |
| Inbound sizes | 1 MiB input messages, 64 KiB others |
| Outbound message | 4 MiB |
| Surface size | cols ≤ 500, rows ≤ 200 |
| Style table / link table | 4096 / 256, then full frame |
| Pushes | 30 per device per minute |

What a paired device can do: everything a local CLI user can do through the
allowlisted API methods, plus typing into any pane. That is Dylan's shell.
Treat a lost phone like a lost SSH key: `flockr mobile devices revoke`.

## 13. Server code layout

Shared by CLI and server (no server runtime needed):

- `src/mobile/mod.rs` — re-exports.
- `src/mobile/store.rs` — state dir paths, `server-id`, `devices.json`,
  `pairing.json`, `gateway.json`; token/id generation, hashing,
  constant-time compare; atomic writes.
- `src/mobile/pairing.rs` — pairing payload JSON + URL encode/decode, QR
  rendering to a string.

CLI:

- `src/cli/mobile.rs` — `pair`, `devices list|revoke`, `status`; wired in
  `src/cli.rs` (`"mobile" =>`) and the help/spec listing.

Server gateway (all tokio tasks on the existing server runtime, started from
`src/server/headless/bootstrap.rs` in both the normal and handoff-import
paths, and stopped through `ServerStop`; a live handoff closes it with `bye`
and the new process re-binds):

- `src/server/mobile/mod.rs` — `MobileGateway::start(config, deps) ->
  Option<MobileGatewayHandle>`; `deps` = `server_event_tx`, `EventHub`, API
  socket path, `ServerStop`. Limits constants.
- `src/server/mobile/listener.rs` — bind/retry per address, accept,
  per-IP rate limits, WebSocket upgrade (path + subprotocol check),
  `gateway.json`.
- `src/server/mobile/connection.rs` — per-connection state machine: hello,
  auth/pair, dispatch, outbound queue, revocation check, app state.
- `src/server/mobile/protocol.rs` — serde types for every message here
  (pure). Unit tests decode/encode JSON fixtures in
  `tests/fixtures/mobile-v1/*.json` (one file per example in this doc), so the
  app repo can copy the same fixtures for its tests.
- `src/server/mobile/cells.rs` — `FrameData` → styles/runs, diff, shift
  detection (pure, unit-tested; add a small bench case for 120×40 and 300×80).
- `src/server/mobile/surface.rs` — subscription bridge: `ServerEvent`
  open/resize/close, in-process writer drain thread, `watch` channel, flow
  control and fps cap.
- `src/server/mobile/api_proxy.rs` — allowlist; forwards each
  `api.request` as one line to the local API socket (a fresh connection per
  request, kept open for streams). This reuses the API server unchanged,
  including `events.subscribe` and the wait methods. Blocking socket I/O runs
  in `spawn_blocking`.
- `src/server/mobile/agents.rs` — `agents.watch` aggregator.
- `src/server/mobile/push.rs` — trigger/debounce/dedupe state machine (pure,
  unit-tested with a fake clock) and the Expo sender thread.
- `src/server/headless/mobile.rs` — handlers for the new internal
  `ServerEvent` variants (open/resize/close mobile surface clients, mobile
  pane input).
- Small additions: `ClientWriter::in_process()` (`client_transport.rs`),
  `ClientRenderState::Cells` (`render_stream.rs`), mobile observe size in
  `render.rs`, `ServerHandle` path accessor if needed (`src/api/server.rs`).

No `#[cfg(target_os)]` in these modules; tokio TCP works on all three
platforms. File modes use the existing `#[cfg(unix)]` permission pattern
(`restrict_socket_permissions`).

## 14. Dependencies

Checked `Cargo.toml`: there is no HTTP client, WebSocket, QR, or TCP support
today (tokio lacks the `net` feature). `sha2`, `base64`, `serde_json`,
`unicode-width`, and `futures-util` (Linux only) exist.

| Crate | Why | Notes |
|---|---|---|
| tokio feature `net` | `TcpListener`/`TcpStream` for the gateway. | Feature flag on the existing dependency. |
| `tokio-tungstenite` | WebSocket server handshake and framing on tokio. | `default-features = false, features = ["handshake"]`. No TLS features. Use `accept_hdr_async` to check path and subprotocol. |
| `futures-util` | `StreamExt`/`SinkExt` for the WebSocket stream. | Move from the Linux-only table to `[dependencies]` (same version, `default-features = false, features = ["std", "sink"]`). |
| `qrcode` | Terminal QR for `flockr mobile pair`. | `default-features = false`; uses only the Unicode renderer. |
| `getrandom` | Token/code/id randomness. | Already in `Cargo.lock` (0.3.4) transitively; direct dep adds no new code. |
| `ureq` | HTTPS POST to the Expo push API. | Blocking client on its own thread; `default-features = false, features = ["rustls"]` (check exact feature names for the chosen 3.x version so it uses rustls with bundled webpki roots). Rejected: shelling out to `curl` (process spawn per push, harder error handling); `reqwest` (larger). |

Each `Cargo.toml` line gets a short comment with its reason, matching the
existing style.

## 15. App notes (flockr-mobile)

- Expo SDK 57, TypeScript. `@shopify/react-native-skia` for the grid,
  `expo-camera` for QR, `expo-secure-store` for tokens. All run in Expo Go,
  so drawing speed can be tested without an EAS build.
- `expo-notifications` + `expo-dev-client` for push: push registration only
  runs in a dev/preview build (`Constants.executionEnvironment !== ExecutionEnvironment.StoreClient`
  (`appOwnership` is deprecated) and a
  physical device); guard it so Expo Go still runs.
  `getExpoPushTokenAsync({ projectId })` with the EAS project id from
  `app.config.js`, as in Corvi.
- Plain `ws://` to a NetBird IP needs an iOS ATS exception
  (`ios.infoPlist.NSAppTransportSecurity.NSAllowsArbitraryLoads = true`) and
  Android cleartext (`expo-build-properties`
  `android.usesCleartextTraffic = true`) in every build profile, not only
  dev builds: v1 has no TLS, so a preview or TestFlight build without them
  cannot connect. Expo Go allows it already.
- Store per server: `{server_id, name, hosts, port, device_id, token}`.
  Try hosts in order on connect; remember the last one that worked.
- Implement the decoder against the JSON fixtures in
  `tests/fixtures/mobile-v1/` (copy them into the app repo's tests).

## 16. Server implementation notes (v1 prototype)

What the Rust server does where this file left room, or where the first
draft changed during implementation:

- Spec fixes made while implementing: the §10.5 example line is `"$"` (the
  server omits trailing default-style spaces, so `"$ "` cannot occur); the
  full-frame rule in §10.4 and the shift rule in §10.5 are stated as built.
- `surface.started.theme` is not sent yet (allowed: "else omitted"). The app
  uses its built-in palette.
- `notify` goes to every device with a live foreground connection, with or
  without a push token. `mobile.push.enabled = false` only stops Expo sends;
  in-band `notify` still works. `push.expo` is listed in `features` only when
  push is enabled.
- `surface.scroll` has no success reply; failures answer `error` when the
  message had an `id`.
- `surface.subscribe` with an unknown `mode` is treated as `observe`.
- `last_seen_at` is written on connect when the last write was more than
  10 minutes ago, and on disconnect.
- The upgrade header cap (8 KiB) is checked after tungstenite parses the
  request.
- State files are per install, not per named session: two named sessions
  with the gateway enabled share `devices.json` and would also need
  different ports.
- After a live handoff the new process re-binds; while the old process still
  holds the port the bind retries every 10 s, so the phone may wait up to
  10 s to reconnect.
- Each subscription renders on the server whenever its pane changes; acks and
  `max_fps` limit what is encoded and sent, not what is rendered. The
  in-process render lane holds one frame, so a slow phone never queues work
  on the server loop.
