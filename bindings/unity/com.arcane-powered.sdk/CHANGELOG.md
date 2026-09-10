# Changelog

This package tracks the version of the native SDK it wraps.

## 0.11.0

### Fixed

- **Ownership checks failed on every machine.** The SDK derived its device
  fingerprint from a random local UUID while the Arcane desktop app derived the
  one it binds a ticket to from the account key, so no ticket could ever match
  and `Arcane.Ownership` reported `device_mismatch` for every DRM-enabled title.
  The desktop now publishes the public key fingerprints and the SDK rehashes
  them itself. Needs an Arcane desktop build from the same release.
- The `dev` claim is read as a string *or* a list, which is what the backend
  mints so that rotating an account key does not invalidate cached tickets.

### Changed

- **A launch always mints a fresh ticket.** `init` asks the desktop app first
  instead of trusting the cached ticket, and falls back to the cache only when
  the desktop says it cannot reach the cloud. A key rotated since the ticket was
  minted no longer locks a player out of a title they own.
- `Arcane.DeviceHash` is per machine **and** per account: the same machine
  reports a different value for a different signed-in account, and it is the
  fingerprint the ticket was actually minted for.

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
