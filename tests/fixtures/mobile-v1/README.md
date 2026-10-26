# mobile.v1 fixtures

One JSON file per example in `docs/mobile/protocol-v1.md`.

- `client.*`: messages the app sends. The server's unit tests decode every one.
- `server.*`: messages the server sends.
- `pairing.payload.json`: the JSON inside the pairing URL.

`server.surface.frame.json` then `server.surface.delta.json` describe a
20×3 grid. After applying both, the app's grid is:

| row | runs |
|---|---|
| 0 | empty (old row 1 shifted up) |
| 1 | `[0,3,"test result: ok"]` |
| 2 | `[0,0,"$"]` |

The app repo copies this directory into its tests and checks its decoder
against it.
