# mobile.v2 fixtures

One JSON file per example in `docs/mobile/protocol.md`.

- `client.*`: messages the app sends. The server's unit tests decode every one.
- `server.*`: messages the server sends.
- `pairing.payload.json`: the JSON inside the pairing URL. Its `pk` is the
  responder static public key from `noise-nk.json`.
- `noise-nk.json`: Noise test vector (§7.1, §7.2). Fixed keys (responder
  static, both ephemerals), handshake messages 1 and 2, the handshake hash,
  and three transport messages in order (initiator, responder, initiator).
  Each `plaintext_hex` is the framed plaintext: flag byte `00` + the JSON
  message. Each `ciphertext_hex` is one WebSocket binary frame. Generated
  with `snow` by `server::mobile::noise::tests::noise_nk_fixture_matches_snow`;
  rerun it with `FLOCKR_BLESS_NOISE_VECTOR=1` to rewrite the file. An app
  replays it by building an initiator with the same prologue, remote static
  key, and ephemeral key, then checking every message byte for byte.

`server.surface.frame.json` then `server.surface.delta.json` describe a
20×3 grid. After applying both, the app's grid is:

| row | runs |
|---|---|
| 0 | empty (old row 1 shifted up) |
| 1 | `[0,3,"test result: ok"]` |
| 2 | `[0,0,"$"]` |

The app repo copies this directory into its tests and checks its decoder
against it.
