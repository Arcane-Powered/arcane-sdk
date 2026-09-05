> For Mintlify product knowledge (components, configuration, writing standards),
> install the Mintlify skill: `bunx skills add https://mintlify.com/docs`

# Documentation project instructions

## About this project

- Docs for **Arcane SDK** (`arcane-sdk`), the native bridge between games and Arcane Powered
- Position the SDK as full platform integration (not “DRM-only”); document ownership as the first shipped surface
- Site root is this `documentation/` directory (`docs.json` here)
- Preview locally with: `bunx mint dev` (from this directory)
- Prefer Bun over npm/pnpm for CLI tooling in this repo

## Terminology

- **Arcane Powered** — the platform / desktop launcher
- **Arcane SDK** — the integration layer games link to talk to Arcane Powered (Rust + C ABI)
- **Game id** — the identifier of a title in the Arcane portal; the Arcane desktop app puts it in `ARCANE_GAME_ID` on the game process, and `ArcaneClient::init` reads it. `init` takes **no argument** — never write that a game passes the id to it
- **Launch environment** — `ARCANE_GAME_ID` and `ARCANE_USER_ID`, set by the desktop app when it launches a game; a developer sets them by hand for local development
- **Client** — the `ArcaneClient` a game builds once at launch; holds the `game_id` it was launched with, plus `user_id`, ownership, device hash
- **Ownership ticket** — cached JWT proving offline ownership for a title + device (one platform surface)
- **DRM root** — `{app_data}/Arcane Powered/drm/`
- **Session** — `session.json` under the DRM root, written by Arcane desktop; names the signed-in account
- Prefer "platform integration" for the SDK overall; ownership is the default launch check inside `ArcaneClient::init`, not a separate integration step
- Call the identifier the **game id** — never a "public key". There is one identifier, not two: `game_id()` returns exactly what `ARCANE_GAME_ID` held. Say the portal value is what a developer sets locally, not what a shipped game passes to `init`
- Building the client *is* the ownership check — never imply a second ownership call is needed
- The client does not revalidate on its own; say so wherever `refresh` is mentioned

## Style preferences

- Use active voice and second person ("you")
- Keep sentences concise — one idea per sentence
- Use sentence case for headings
- Bold for UI elements: Click **Settings**
- Code formatting for file names, commands, paths, and code references
- Code samples: Rust first, then C for FFI; use Bun in shell examples when showing JS tooling

## Content boundaries

- Document public Rust API and C ABI only — public surface is `ArcaneClient` (`init`, `refresh`, accessors), the `arcane_sdk_*` C functions, plus `OwnershipStatus` / `SdkError` / `ErrorCode`
- Do not document internal helpers (paths, device hash, raw JWT verify) as integration APIs
- Do not invent achievements / cloud saves / friends APIs until they exist in `src/`
- Document that `init` may contact the desktop loopback (`127.0.0.1:39284`) and open `arcane-powered://` when refreshing; do not document other launcher-internal endpoints as game APIs — the desktop request/response contract lives in [`DESKTOP_CONTRACT.md`](../DESKTOP_CONTRACT.md) at the repo root, deliberately outside this site
- `ARCANE_GAME_ID` and `ARCANE_USER_ID` are the two variables **expected in production**: the desktop app sets them, and a developer may set them by hand for local development. Document them on [`concepts/local-development.mdx`](./concepts/local-development.mdx)
- The `ARCANE_DRM_ROOT` / `ARCANE_SDK_PORT` / `ARCANE_OFFLINE_ONLY` / `ARCANE_SESSION_TICK_MS` env vars are developer/QA tooling — always say they must never be set in a shipped game
- Keep security-sensitive details accurate to source; do not add speculative bypass guidance
- Keep [`contributing.mdx`](./contributing.mdx) aligned with `.github/workflows` (tag-only release, version bump in PR, merge queue)

## Errors in docs

- [`concepts/errors.mdx`](./concepts/errors.mdx) is the source of truth for codes and fixes
- Every public function page must list **which codes that function can return** and link to Errors (debug must be: this function → this code → this problem)
- Avoid duplicating the full fix table on reference pages — short code/problem table + link is enough
- `SdkError` has four parts: `code` (match on it), `message` (safe to show a player), `hint` (what the developer should do), `context` (the values involved). Keep that distinction in prose — do not describe `message` as the whole error
- Keep the two C ABI return conventions clearly separated: actions return `0`/`1`/`2`, getters return bytes-written or a negative code
