# Changelog

This package tracks the version of the native SDK it wraps.

## 0.10.0

First release.

- `Arcane` — a static façade over the whole C ABI: ownership, identity, the play
  session, achievements, friends and lobbies.
- `ArcaneRuntime` — initialises before the first scene, counts frames, reports
  graphics settings, pumps lobby events onto the main thread, and ends the
  session on quit and on leaving play mode.
- `ArcaneSettings` — every one of those jobs as a checkbox, plus the game id and
  account the Editor runs under, which the package puts in the process
  environment where the native SDK reads them.
- `ArcaneError` — the SDK's stable codes as an enum, with the hint and context
  behind them. A code newer than this package keeps its wire string.
- `Try…` / throwing pairs for everything that can fail, and `…Async` twins for
  everything that blocks on the Arcane desktop app.
- Lobby payloads as `byte[]`; the base64 the C ABI wants never reaches your code.
- An importer that points each native plugin at the platform it was built for.
