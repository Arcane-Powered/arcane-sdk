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
| Names a user | Reads exactly `tickets/{user_id}/{game_id}.ticket`. **No fallback** — another account's ticket must never satisfy this one. |
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

## 3. `POST /v1/games/{game_id}/ownership/refresh` — add `user_id`

**Status: new field, additive.**

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
| `game_id` | optional | Echo of the `{game_id}` path segment. The SDK ignores it: `ArcaneClient::game_id()` is the value the game passed to `init`. |

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
arcane-powered://sdk/ownership?game_id={game_id}
```

Opened when the loopback is not reachable. The SDK then polls `/v1/health` every
400 ms for up to 25 s before giving up with `arcane_unavailable`.

The game id is interpolated raw, but the SDK validates it against
`[A-Za-z0-9_.-]{1,256}` before any URL is built, so it cannot carry a query
separator, a path traversal, or whitespace.

---

## 5. On-disk layout the SDK reads

```
{app_data}/Arcane Powered/drm/
├── machine_id                              written by the SDK (mode 0600)
├── jwks.json                               written by the desktop
├── session.json                            written by the desktop  ← new
├── flags/{game_id}.json                    { "drm_enabled": bool }
└── tickets/{user_id}/{game_id}.ticket
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

The SDK reads `user_id` from this file to populate the client, so it should be
filled even when `drm_enabled` is `false`. `game_id` stays in the layout for the
desktop's own use; the SDK does not read it, since the game already told `init`
which title it is.

JWT claims enforced by the SDK: `gid` must equal the game id passed to `init`,
`own` must be `true`, `dev` must equal the local device hash, `iss` =
`arcane-drm`, `aud` = `arcane-game-sdk`, ES256, ±300 s skew on `iat`/`nbf`/`exp`.

---

## 6. Not in scope: verifying the calling process

The game id is **not a secret and is not a credential**. It is a public
identifier — it names a title, exactly like the `{game_id}` already in every
route above — and knowing it grants nothing. Ownership is proven by the signed
ownership ticket alone: an ES256 JWT minted by the backend, bound to an account
and to a device, which no caller can forge from the id. A desktop build must
never treat the id in a request as evidence of anything beyond which title is
being asked about.

The SDK does **not** attempt to prove that the process calling the loopback is
the game matching the game id, and it should not: any secret compiled into a
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
| `ARCANE_OFFLINE_ONLY` | `1`/`true`: never contact or launch the desktop app. Can only make a check fail earlier, never let one pass. Also disables the session thread entirely. |
| `ARCANE_SESSION_TICK_MS` | Replaces the 60 s session heartbeat period, so a test does not have to wait a minute. Does not change the FPS window schedule. |

---

## 8. Game sessions

**Status: new. Required for playtime and FPS; absent routes degrade, they never fail `init`.**

All routes are on `http://127.0.0.1:39284`, JSON bodies, `{ "error", "message", "details?" }`
error bodies as everywhere else. The SDK maps a `404` **without** a JSON body
(unknown route, desktop too old) to `feature_unavailable`.

```
POST /v1/games/{game_id}/session/start
→ 200 { "session_id": "uuid", "user_id": "…", "game_id": "…", "fps_sampling": true }
→ 401 not_authenticated · 403 not_owned · 404 game_not_found

POST /v1/games/{game_id}/session/heartbeat
{ "session_id": "uuid", "seconds": 120,
  "samples": [ { "sample_id": "uuid", "taken_at": 1786480000, "fps_avg": 59.8,
                 "window_seconds": 30, "frames": 1794,
                 "resolution": "2560x1440", "graphics_preset": "high" } ] }
→ 200 { "ok": true, "fps_sampling": true }
→ 404 unknown_session (the desktop expired the session → the SDK starts a new one)

POST /v1/games/{game_id}/session/end
{ "session_id": "uuid", "seconds": 1830, "samples": [ … ] }
→ 200 { "ok": true }
```

- `seconds` is **cumulative since the start of the session**, not a delta: a lost or
  replayed heartbeat changes nothing, the desktop keeps the max.
- `samples`: the windows closed since the last **acknowledged** heartbeat (usually 0
  or 1). Each sample carries a `sample_id` generated by the SDK so the desktop and
  the backend can deduplicate a re-send. `[]` when `frame()` is never called or when
  `fps_sampling` is `false`.
- `resolution` / `graphics_preset`: optional, the value of `set_graphics()` at the
  time of the window. Omitted when the game never called it.
- `fps_sampling` reflects the player's setting in the desktop app; the SDK applies it
  as soon as it reads the response.
- The desktop expires a session after 3 missed heartbeats (180 s) and flushes what it
  has.

### What the SDK does around this

| Situation | SDK behaviour |
|---|---|
| Desktop unreachable at `init` | `init` still succeeds on ownership alone. Tracking is `Pending`, `session/start` is retried every 60 s. The deep link is **never** opened for a session. |
| Desktop older than these routes | `404` without a JSON body → `feature_unavailable`, tracking stays `Pending`, retried silently. |
| `unknown_session` on a heartbeat | The SDK drops the session id and starts a new session immediately. |
| Never reachable for the whole run | That session's playtime is lost. The SDK buffers nothing on disk. |

---

## 9. Achievements

**Status: new. Absent routes degrade to `feature_unavailable`; they never fail `init`.**

Same conventions as §8: `http://127.0.0.1:39284`, JSON bodies, `{ "error", "message",
"details?" }` error bodies. A `404` **without** a JSON body (unknown route, desktop too
old) maps to `feature_unavailable`.

```
GET /v1/games/{game_id}/achievements
→ 200 { "achievements": [ { "key": "first_blood", "title": "…", "description": "…",
         "icon_url": "…|null", "hidden": false, "unlocked_at": "2026-…|null" } ] }

POST /v1/games/{game_id}/achievements/{key}/unlock
→ 200 { "key": "first_blood", "unlocked_at": "…", "already_unlocked": false, "queued": false }
→ 404 unknown_achievement · 403 not_owned
```

- `unlocked_at` is RFC 3339 on the wire. The SDK exposes it as a Unix timestamp
  (`i64`); a timestamp it cannot parse reads as "still locked" rather than a wrong date.
- `queued: true` means the desktop app is offline and has stored the unlock for later.
  The SDK treats it as a success and updates its cache.
- The unlock is **idempotent**: a repeat answers `200` with `already_unlocked: true`,
  carrying the original `unlocked_at`. A game may call it every time its condition holds.
- `{key}` is interpolated raw, and the SDK validates it against the same charset the
  backend and the desktop enforce — `^[a-z0-9_.-]{1,64}$`, minus keys made only of dots
  (`.`, `..`) which an HTTP client would normalise into a different route — before
  building the URL, so an invalid key never leaves the process (`invalid_argument`).
  A `400 invalid_key` from the desktop maps to the same code.
- Both calls are synchronous on the calling thread, one round trip each. The SDK never
  polls achievements in the background and never opens the deep link for them.
- `GET /achievements` is not required to reflect an unlock that is still queued, so a
  game that lists right after an offline unlock may see that key locked again until the
  queue drains. The SDK's own cache keeps the unlock until the next `list`.

### What the SDK does around this

| Situation | SDK behaviour |
|---|---|
| Desktop unreachable | `arcane_unavailable` on both calls. Nothing is retried, nothing is buffered on disk |
| Desktop older than these routes | Bare `404` → `feature_unavailable` |
| `404 unknown_achievement` | `unknown_achievement`, with the key in the error context |
| `ARCANE_OFFLINE_ONLY` set | `network_required`, raised before any call |
| `list` never called | `is_unlocked` answers `None` — the SDK does not guess |

---

## 10. Friends

**Status: new. Absent routes degrade to `feature_unavailable`; they never fail `init`.**

Same conventions as §8 and §9: `http://127.0.0.1:39284`, JSON bodies, `{ "error",
"message", "details?" }` error bodies. A `404` **without** a JSON body (unknown route,
desktop too old) maps to `feature_unavailable`.

```
GET /v1/friends
→ 200 { "friends": [ { "user_id": "…", "pseudo": "…", "online": true,
         "playing_game_id": "…|null" } ], "stale": false }
→ 401 not_authenticated
```

- The route is **not** scoped to a title: it answers for the signed-in account, and the
  SDK derives `in_game = playing_game_id == game_id()` per friend, where `game_id()` is
  the id the game passed to `init`.
- `stale: true` means the desktop app is offline and served its own cache. The SDK
  passes it through as `FriendList::stale`; it is a successful answer, not an error.
- The desktop app caches the list for 15 seconds. The SDK caches nothing and calls the
  route every time the game asks, so the cache is the desktop's to size.
- `playing_game_id` is the `game_id` of the title the friend is playing — the same value
  a game passes to `init` and the SDK puts in the `{game_id}` routes. The desktop sets
  that presence itself, so nothing here depends on the game.
- One synchronous round trip on the calling thread. The SDK never polls friends in the
  background and never opens the deep link for them.
- Friend requests, chat and the overlay are launcher flows. The SDK only reads presence.

### What the SDK does around this

| Situation | SDK behaviour |
|---|---|
| Desktop unreachable | `arcane_unavailable`. Nothing is retried, nothing is buffered on disk |
| Desktop older than this route | Bare `404` → `feature_unavailable` |
| `401 not_authenticated` | `not_authenticated`, retryable — the player can sign in and retry |
| `ARCANE_OFFLINE_ONLY` set | `network_required`, raised before any call |
| A friend is playing another title | Their `in_game` is `false`; `online` still comes through |

---

## 11. P2P lobbies

**Status: new. Absent routes degrade to `feature_unavailable`; they never fail `init`.**

Same conventions as §8–§10: `http://127.0.0.1:39284`, JSON bodies, `{ "error", "message",
"details?" }` error bodies. A `404` **without** a JSON body (unknown route, desktop too
old) maps to `feature_unavailable`.

```
POST /v1/games/{game_id}/lobbies
{ "max_players": 4, "visibility": "friends" | "code" | "friends_and_code", "payload": "<base64 ≤ 4 KiB>" }
→ 200 { "lobby_id", "join_code": "K7P3QX" | null, "host_user_id", "host_payload", "visibility", "max_players",
         "members": [ { "user_id", "pseudo", "payload" } ], "expires_at" }

POST /v1/games/{game_id}/lobbies/join       { "join_code": "K7P3QX", "payload": "…" }   → 200 same lobby object
POST /v1/games/{game_id}/lobbies/{id}/join  { "payload": "…" }                          → 200 same lobby object
GET  /v1/games/{game_id}/lobbies/{id}                                                   → 200 same lobby object
→ 404 lobby_not_found · 409 lobby_full · 410 lobby_closed · 403 not_friends

POST /v1/games/{game_id}/lobbies/{id}/invite  { "to_user_id": "…" }   → 200 { "ok": true }
POST /v1/games/{game_id}/lobbies/{id}/leave                             → 200 { "ok": true }
DELETE /v1/games/{game_id}/lobbies/{id}                                  → 200 { "ok": true }  (host)

GET  /v1/games/{game_id}/lobbies/events?after={cursor}
→ 200 { "events": [ { "id": "…", "type": "invite" | "member_joined" | "member_left" | "lobby_closed",
         "lobby_id", "join_code": "…|null", "from_user_id": "…|null", "user_id": "…|null",
         "pseudo": "…|null", "payload": "…|null" } ], "cursor": "…", "dropped": false }

GET  /v1/games/{game_id}/launch-context   → 200 { "join_code": "K7P3QX" | null }
```

- Arcane provides the **meeting point only**. There is no relay, no NAT traversal, no
  transport and no host migration in the SDK, and no public lobby list: game traffic is
  the game's own netcode. The lobby carries who is in it and one opaque blob per member.
- `payload` is that blob — an address, a ticket from the game's netcode, whatever it
  wants. It is **base64 (standard alphabet, padded)** on the wire, at most **4096 raw
  bytes**. The SDK refuses a longer one with `invalid_argument` before any call, and
  never interprets the content. A payload the desktop app cannot relay verbatim fails
  the lobby call with `arcane_unavailable`; inside an event it arrives empty, since the
  session thread has no caller to report to.
- `join_code`: 6 characters from `[A-HJ-NP-Z2-9]` (no `I`, `O`, `0`, `1`), unique among a
  title's open lobbies, generated by the backend. The SDK uppercases what the player
  typed and validates the shape before any call. `join_code` is `null` in a lobby object
  for a non-host member of a `friends` lobby.
- `{id}` and `to_user_id` are interpolated raw, and the SDK validates them as
  `^[A-Za-z0-9-]{1,64}$` first, so a malformed id never leaves the process
  (`invalid_argument`).
- The lobby ends when the host leaves, calls `DELETE`, or their play session (§8)
  expires. Members get a `lobby_closed` event; nothing is migrated.
- `/lobbies/events` is the only polled route in the SDK, and only once the game has
  called `p2p()` at least once. `cursor` is opaque: the first call omits `after`, and
  every later call sends back the `cursor` of the previous answer. The desktop app must
  deliver each event **once** for a given cursor chain; the SDK also drops an `id` it
  has already delivered, so a replay costs the game nothing.
- `dropped: true` means the desktop app's ring buffer evicted events this client never
  fetched. The SDK turns it into one `LobbyEvent::Resync` (`"type": "resync"` in the C
  ABI), delivered **before** the events of that same answer, and the cursor keeps its
  usual meaning. A game that gets one re-reads the lobbies it is in with `GET
  /lobbies/{id}` rather than trusting what the earlier events built up.
- `GET /lobbies/{id}` answers the same lobby object as create and join, for exactly
  that: reading a lobby without joining or leaving anything.
- A `payload` longer than 4096 raw bytes is refused on the way in as well as on the way
  out: it fails a lobby object with `arcane_unavailable` and arrives empty inside an
  event. A lobby object without a `lobby_id` is refused the same way.
- `launch-context` is set by the launcher when it starts the game from a friend's "Join",
  and the desktop app **clears it once served**. The SDK reads it at most once per
  client and caches the answer.

### What the SDK does around this

| Situation | SDK behaviour |
|---|---|
| Game never calls `p2p()` | Nothing is polled and no lobby route is ever called. `session().lobby_events` is `off` |
| `p2p()` called | The `arcane-session` thread polls `/lobbies/events` on every tick — every 5 s while the client is in an open lobby, 60 s otherwise. Heartbeats keep their own 60 s schedule |
| Desktop older than these routes | Bare `404` on `/lobbies/events` disarms polling **silently** and for good: `session().lobby_events` becomes `unavailable` and nothing else is requested. The game-facing calls return `feature_unavailable` |
| Desktop unreachable | `arcane_unavailable` on the game-facing calls; the poll records the failure and retries on the next tick |
| `404 lobby_not_found` · `409 lobby_full` · `410 lobby_closed` · `403 not_friends` | The matching SDK code, with `lobby_id` or `join_code` in the error context |
| `ARCANE_OFFLINE_ONLY` set | `network_required` on every lobby call, raised before any request; `launch_join_code()` answers `None` |
| Events never polled by the game | The queue keeps the 256 most recent events and drops the oldest |
