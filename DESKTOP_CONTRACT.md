# Arcane desktop ↔ SDK contract

What the Arcane Powered desktop app must provide for `arcane-sdk` to work.

This is an **internal** contract between the launcher and the SDK. It is
deliberately not on the Mintlify docs site — games never call these endpoints
directly, and documenting them as game APIs would invite exactly that.

The SDK is written so it **works with today's desktop build without any change**.
Everything marked *new* below is additive: absent fields simply leave the
corresponding client state as `None`.

---

## 1. `session.json` — required for correct multi-account behaviour

**Status: new. This is the one item the SDK genuinely needs.**

```
{app_data}/Arcane Powered/drm/session.json
```

```json
{ "user_id": "3f2a…", "updated_at": 1786480000 }
```

- Written **on sign-in**, with the account's cloud user id.
- Rewritten **on sign-out** with `"user_id": null` (or the key omitted).
- Must be kept in sync with `tickets/{user_id}/`.

### Why

The SDK cannot ask the desktop who is signed in while **offline**, and offline is
exactly when the ownership ticket matters. `session.json` is the only offline
source of truth.

Without it the SDK falls back to scanning `tickets/*/` for a matching ticket —
which is what the pre-0.4 SDK always did, and it is wrong on a shared machine:

> User A buys the title and signs out. User B signs in. A's ticket is still on
> disk, the scan finds it first, and B plays on A's entitlement. The ticket is
> correctly signed and correctly bound to the device, so nothing rejects it.

### How the SDK behaves for each state

| `session.json` | SDK behaviour |
|---|---|
| Names a user | Reads exactly `tickets/{user_id}/{public_key}.ticket`. **No fallback** — another account's ticket must never satisfy this one. |
| Present, `user_id` null/empty | `not_authenticated` — nobody is signed in, so no ownership can be attributed. |
| Absent (older desktop) | Scans `tickets/*/`. Exactly one match is used. Several matches → `ambiguous_session` rather than a guess. |

---

## 2. `GET /v1/health` — add `user_id`

**Status: new field, additive.**

```json
{ "ok": true, "authenticated": true, "user_id": "3f2a…" }
```

`user_id` is *optional*. When present the SDK uses it to populate
`ArcaneClient::user_id()` on paths where no ticket file was read (for example
when DRM is disabled for the title).

Existing fields are unchanged and still required:

- `ok` — the loopback SDK server is healthy. `false` → SDK returns `arcane_unavailable`.
- `authenticated` — somebody is signed in. `false` → SDK returns `not_authenticated`
  **before** it sends the refresh request.

---

## 3. `POST /v1/games/{public_key}/ownership/refresh` — add `user_id` and `game_id`

**Status: new fields, additive.**

The SDK sends no request body.

### Success (2xx)

```json
{
  "ok": true,
  "drm_enabled": true,
  "user_id": "3f2a…",
  "game_id": "canonical-title-id"
}
```

| Field | Required | Meaning |
|---|---|---|
| `ok` | yes | Refresh completed. |
| `drm_enabled` | yes | Whether this title enforces DRM. |
| `user_id` | *new*, optional | Signed-in account. Takes precedence over the `/v1/health` value. |
| `game_id` | *new*, optional | Canonical title id, surfaced as `ArcaneClient::game_id()`. |

Two behaviours worth knowing:

- `ok: false` **with** `drm_enabled: true` → the SDK fails with `ticket_invalid`.
- `ok: false` **with** `drm_enabled: false` → treated as success. There is no
  ticket to mint, so nothing failed. (This matches the pre-0.4 SDK.)
- When `drm_enabled` is `false` and no ticket file appears on disk afterwards,
  the SDK now resolves to `DrmDisabled` instead of failing with `ticket_missing`.

### Failure (non-2xx)

```json
{ "error": "not_owned", "message": "This account does not own the requested game." }
```

`error` must be one of the codes below. `message` is developer-facing detail and
is carried into the SDK error's `context.detail`.

| `error` | SDK code | Retryable |
|---|---|---|
| `not_owned` | `not_owned` | no |
| `offline` | `network_required` | yes |
| `not_authenticated` | `not_authenticated` | yes |
| `game_not_found` | `ticket_invalid` | no |
| `cloud_unreachable` | `network_required` | yes |
| `cloud_error` | `network_required` | yes |
| `internal` | `internal` | no |
| *anything else* | `ticket_invalid`, with the original string in `context.desktop_error` | no |

New `error` values are safe to introduce: an older SDK degrades to
`ticket_invalid` and preserves the original code in its error context.

---

## 4. Deep link

```
arcane-powered://sdk/ownership?game_id={public_key}
```

Opened when the loopback is not reachable. The SDK then polls `/v1/health` every
400 ms for up to 25 s before giving up with `arcane_unavailable`.

The `public_key` is interpolated raw, but the SDK validates it against
`[A-Za-z0-9_.-]{1,256}` before any URL is built, so it cannot carry a query
separator, a path traversal, or whitespace.

---

## 5. On-disk layout the SDK reads

```
{app_data}/Arcane Powered/drm/
├── machine_id                              written by the SDK (mode 0600)
├── jwks.json                               written by the desktop
├── session.json                            written by the desktop  ← new
├── flags/{public_key}.json                 { "drm_enabled": bool }
└── tickets/{user_id}/{public_key}.ticket
```

Ticket file:

```json
{
  "ticket": "<ES256 JWT>",
  "cached_at": "2026-01-01T00:00:00Z",
  "expires_at": "2027-01-01T00:00:00Z",
  "game_id": "canonical-title-id",
  "user_id": "3f2a…",
  "device_hash": "…",
  "drm_enabled": true,
  "last_seen_wall_time": 1786480000
}
```

The SDK reads `user_id` and `game_id` from this file to populate the client, so
both should be filled even when `drm_enabled` is `false`.

JWT claims enforced by the SDK: `gid` must equal the public key, `own` must be
`true`, `dev` must equal the local device hash, `iss` = `arcane-drm`,
`aud` = `arcane-game-sdk`, ES256, ±300 s skew on `iat`/`nbf`/`exp`.

---

## 6. Not in scope: verifying the calling process

The SDK does **not** attempt to prove that the process calling the loopback is
the game matching the public key, and it should not: any secret compiled into a
game binary is extractable, and an attacker can simply patch out the init call.
Real enforcement belongs on the server, where entitlement checks cannot be
edited by the client.

Note for the desktop team, separately from the SDK: the loopback currently has
no authentication of any kind. Any local process — including a web page issuing
`fetch` against `127.0.0.1:39284` — can call these endpoints. Whether that
matters is a desktop-side decision, but it is worth being deliberate about
before endpoints beyond ownership are added.

---

## 7. Developer/QA environment overrides

Read by the SDK. Never set by a shipped game.

| Variable | Effect |
|---|---|
| `ARCANE_DRM_ROOT` | Replaces `{app_data}/Arcane Powered/drm`. |
| `ARCANE_SDK_PORT` | Replaces the loopback port `39284`. |
| `ARCANE_OFFLINE_ONLY` | `1`/`true`: never contact or launch the desktop app. Can only make a check fail earlier, never let one pass. |
