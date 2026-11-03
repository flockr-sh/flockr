# flockr app

The flockr phone and web app, in `app/` of the flockr repo. Watch agents,
read their panes and type into them from an iPhone or a browser. Expo SDK 58
+ TypeScript. Android uses the same code. The web build (`npm run export:web`)
is the flockr web app.

The wire contract is `mobile.v2`: the messages in
[`docs/mobile/protocol.md`](../docs/mobile/protocol.md), carried inside a
`Noise_NK_25519_ChaChaPoly_SHA256` channel pinned to the server key from the
pairing QR (path `/mobile/v2`, subprotocol `flockr.mobile.v2`, binary frames
only). Phones paired under v1 must pair again.

## How it works

- The flockr server renders each pane into a cell grid and sends it over a
  WebSocket as JSON: a full frame, then deltas (changed rows, optional
  whole-grid shift). The phone runs no terminal emulator and no SSH.
- Every WebSocket frame is a Noise transport message (`src/protocol/noise.ts`,
  pure JS on `@noble`). The phone is the NK initiator: it knows the server's
  static key from the QR, so only that machine can finish the handshake. The
  device token and pair code travel inside the channel. A failed handshake
  stops reconnecting and asks the user to pair again; the token is kept.
- The app draws the grid with `@shopify/react-native-skia`: one recorded
  picture per row, re-recorded only when that row changes, plus a cheap
  composite picture per drawn frame.
- Flow control: the app acks each update after handing the picture to the
  native view. The server keeps at most 2 updates in flight, so its send rate
  settles at what the phone can draw.

## Layout

```
App.tsx                  screen stack, push wiring, in-app alert banner
src/protocol/            mobile.v2 client; no React imports, runs under Node
  types.ts               message types, close codes
  pairing.ts             flockr://pair URL decode, host parsing, utf8/base64url
  noise.ts               Noise NK initiator (+ responder for the mock), v2
                         framing (flag byte + chunks of at most 65518 bytes)
  grid.ts                cell grid reducer (frame/delta/shift), colors, styles
  client.ts              GatewayClient: handshake, reconnect, API, agents,
                         surfaces, input, push, ping; Subscription
  stats.ts               rolling windows for the perf overlay
  agents.ts              agent sorting and labels
src/polyfills.ts         crypto.getRandomValues from expo-crypto for Hermes
src/state/               connections (one client per server), secure storage,
                         push (guarded), hooks, UI tokens
src/screens/             Servers, Pair, Agents, Terminal
src/terminal/            Skia renderer, TerminalView (gestures), Composer,
                         PerfOverlay
mock/                    mock gateway (Node + ws) with a fake terminal and a
                         server-side encoder that follows the spec
scripts/mock-gateway.ts  run the mock for the phone
scripts/smoke.ts         drive the protocol client from Node
tests/fixtures/mobile-v1 one JSON file per example in the spec
__tests__/fixtures/      Noise NK test vectors (cacophony, snow)
../tests/fixtures/       shared with the server's Rust tests: mobile-v2/
                         (noise-nk.json, pairing payload) and prompts/
__tests__/protocol/      jest: pairing, grid, agents, encoder round trip,
                         Noise vectors and framing, client against the mock
```

## Commands

```bash
npm install
npm test                 # jest (protocol + mock gateway), Node environment
npm run typecheck        # tsc --noEmit
npx expo-doctor
npx expo export --platform ios   # proves the bundle builds
npm run smoke -- --mock          # protocol client vs in-process mock
npm run smoke -- --flockr <path/to/flockr>   # end-to-end vs a real flockr server
npm run mock-gateway -- --host <ip>   # mock server for the phone
npx tsx scripts/push-check.ts --flockr <bin> --pane <shell pane> --log <flockr-server.log>   # real Expo push path
npm run export:web       # web build in dist/
scripts/emu.sh boot|open|shot|tap|swipe|text|key|stop   # drive the Android emulator
scripts/ios.sh boot|open|snap|tap|tapl|tapxy|touch|swipexy|type|shot|stop   # drive the iOS Simulator on the Mac over ssh
```

`push-check` registers a fake Expo token and fakes a blocked status on a plain
shell pane; it passes when the server log shows Expo's reply (Expo rejects the
fake token as `DeviceNotRegistered` and the server clears it).

`--flockr` runs the given binary for `mobile pair --no-wait --json`,
`pane report-agent`, `pane release-agent` and `mobile devices revoke`, so run it with the same
environment as the server under test (for a debug build: `XDG_CONFIG_HOME` /
`XDG_STATE_HOME` of that server, `FLOCKR_SOCKET_PATH` unset). It pairs, checks
API passthrough, agents, observe and fit (45x30) surfaces, typed text, keys,
submit, resize, scroll, history, push register, an in-band blocked alert, and
revoke. It also prints frame and delta sizes. It changes the target pane's
agent state to blocked and back to working, so point it at a throwaway server.

## Run in Expo Go on an iPhone

Expo Go has Skia, the camera and secure store built in, so the drawing test
needs no build. Push does not work in Expo Go (see below).

1. Install Expo Go on the iPhone.
2. The phone must reach two things on the computer: Metro (port 8081) and the
   gateway (port 47390). Use the computer's NetBird IP (or LAN IP):

   ```bash
   # Metro must advertise an address the phone can reach.
   REACT_NATIVE_PACKAGER_HOSTNAME=100.92.14.3 npm run start:go
   ```

   `start:go` is `expo start --go`; plain `expo start` opens the dev-client
   mode because `expo-dev-client` is installed. Scan the QR that Metro prints
   with the iPhone camera; it opens in Expo Go. If the phone cannot reach
   Metro, `npx expo start --go --tunnel` works from anywhere (slower reloads).
3. Pair with a server (next section) or with the mock.

### Try it without a flockr server

```bash
npm run mock-gateway -- --host 100.92.14.3      # your NetBird or LAN IP
```

It prints a pairing QR and link (a new Noise key each run, so pair again
after restarting it). Panes: `Claude` (static TUI with colors,
wide chars, emoji), `Codex` (log at `--log-rate` lines/s), `shell` (echoes
what you type), `stress` (120x50 full-screen redraw at `--stress-fps`, default
30). Type `b`, `d`, `w` + Enter in the mock's terminal to flip Codex to
blocked/done/working (shows the in-app alert when the app is open).

## Pair with flockr

1. In `~/.config/flockr/config.toml` (`flockr-dev` for debug builds):

   ```toml
   [mobile]
   enabled = true
   listen = ["100.92.14.3"]   # your NetBird address; never 0.0.0.0
   ```

   Restart the flockr server.
2. Run `flockr mobile pair`. It prints a QR code and a `flockr://pair?p=...`
   link, valid for 10 minutes and one use.
3. In the app: **Pair** → scan the QR, or paste the link. The app stores the
   device token in the iOS Keychain (`expo-secure-store`).
4. `flockr mobile devices` lists paired phones; `flockr mobile devices revoke
   <id>` cuts one off (the app then asks to pair again).

The WebSocket is plain `ws://`, but every message inside it is end-to-end
encrypted with Noise and pinned to the server key in the QR. If the server's
key changes (or the address now points at another machine) the app shows
"This server's key changed or the address is a different machine. Pair
again." and stops reconnecting.

You can pair several servers; each one gets its own row on the first screen.

## Using the terminal screen

- Opens in **observe** mode: the pane at its desktop size, scaled to the phone
  width (**Width**). Pinch to zoom, drag sideways to pan. Drag up/down to
  scroll the pane's scrollback (this moves the shared viewport; the desktop
  sees it). **⤓** jumps back to the bottom.
- **Fit: on** resizes the pane's PTY to the phone's grid (agent TUIs reflow to
  phone width). The desktop gets its size back when you leave. **A−/A+** pick
  the font size and so the grid size.
- **History** shows the last 500 lines as plain text.
- Composer: native text field (keyboard, dictation, autocorrect). **Send**
  sends the text once, then Enter. Long-press Send sends the text without
  Enter. The key row sends Esc, Tab, Shift-Tab, Ctrl-C, Ctrl-D, arrows,
  Backspace, Enter, y/n/1/2/3.

## Reading the perf overlay

**Perf** toggles it (on by default). Long-press **Perf** toggles Skia's own
native render-time readout on the canvas.

| Line | Meaning |
|---|---|
| `draw N/s` | Composite pictures handed to the native view per second (the frame rate you see, capped by the display). |
| `js N fps (max gap)` | `requestAnimationFrame` callbacks per second and the longest gap in 2 s. Below 60 or long gaps = JS thread busy. |
| `recv N/s (full N)` | Frames + deltas applied per second; full frames among them. |
| `KB/s` | Surface JSON bytes received (UTF-8) per second. `link` on the last line is all traffic. |
| `rows N/s` | Grid rows replaced per second. |
| `parse` | Average `JSON.parse` ms per surface message (2 s window). |
| `apply` | Average grid reducer ms per message. |
| `record` | Average ms to re-record the changed row pictures. |
| `compose` | Average ms to build the composite picture per drawn frame. |
| `worst` | Largest parse + apply + record in 2 s. |
| `rtt ping` | App-level `ping`/`pong` round trip (every 5 s). |
| `input.ok` | Time from sending an input message to the server's `input.ok`. |
| `echo` | Time from sending input to the next applied update that changed rows or the cursor. Usually the echo of what you typed; on a busy pane it can be unrelated output, so read it on a quiet pane. |
| `resyncs` | Deltas dropped for a `base` mismatch (should stay 0). |

All timings use `performance.now()` on the JS thread. The native draw itself
is not timed here; use the Skia readout (long-press Perf) for that.

What to look at: on the `stress` pane (or a busy real pane), `draw` and
`recv` should track the server rate (30/s by default, up to `max_fps`) with
`js` near 60. If `recv` drops below the server rate, flow control is holding
the server back because the phone is behind; the `parse`/`apply`/`record`/
`compose` numbers say where the time goes.

## Push notifications (dev build)

Expo Go cannot receive remote push, so push code is guarded: in Expo Go the
app never loads `expo-notifications` and shows a note instead. Blocked/Done
alerts still show as an in-app banner while the app is open (server sends
`notify` in-band when the app is in the foreground).

For real pushes, make a development build with EAS, as in Corvi:

1. `npm i -g eas-cli` and `eas login`.
2. `eas init` to create the EAS project. Paste the project id into
   `EAS_PROJECT_ID` in `app.config.js` (Corvi keeps it in `extra.eas.projectId`
   the same way). Add `owner` if the project lives under an organization.
3. Register the iPhone once: `eas device:create`.
4. `eas build --profile development --platform ios`. Install it from the link
   EAS prints. EAS sets up the push key (APNs) when it asks; say yes.
5. Start Metro for the dev client: `REACT_NATIVE_PACKAGER_HOSTNAME=<ip> npm start`.
6. Open the app, allow notifications. It gets an Expo push token
   (`getExpoPushTokenAsync({ projectId })`) and sends it to every paired
   server with `push.register`.
7. Enable push on the server (`[mobile.push] enabled = true`, the default).

Tapping a notification opens that agent's pane. Profiles in `eas.json`:
`development` (dev client, internal), `preview` (internal), `testflight`
(store, auto-increment). The bundle id is `sh.flockr.app` (`.dev` suffix for
the development variant); change it in `app.config.js` if needed.

`app.config.js` turns on the iOS ATS exception and Android cleartext traffic
for every variant because the socket is plain `ws://` (the payload is Noise
encrypted).

## TestFlight

`scripts/testflight.sh` syncs the app to the Mac (ssh host `mac`), runs
`expo prebuild`, and uploads a dev-client build with `fastlane ios beta`.
It is run by hand. Export `ASC_KEY_ID`, `ASC_ISSUER_ID`, `APPLE_TEAM_ID`, and
`APPLE_ID` first; the script passes them to fastlane on the Mac. The App Store
Connect `.p8` key, signing keychain, and keychain password stay in
`~/.flockr-build` on the Mac. `scripts/asc.sh` runs on the Mac and needs
`ASC_KEY_ID` and `ASC_ISSUER_ID` there.

## What is stubbed or missing

- Text selection, copy and link tapping in the grid.
- Mouse input (not in the protocol).
- Glyphs missing from JetBrains Mono fall back to system fonts only for
  non-Latin text; colored emoji depend on the system font match.
- Underline styles: curly/dotted/dashed draw as a single line.
- Blink is drawn steady.
- The mock gateway has no scrollback viewport (`surface.scroll` is ignored)
  and only answers a few API methods.
