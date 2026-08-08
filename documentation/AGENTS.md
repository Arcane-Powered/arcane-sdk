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
- **Portal public key** — identifier generated in the Arcane portal for a title; pass it to `arcane_init` (crate param still named `game_id` today)
- **Ownership ticket** — cached JWT proving offline ownership for a title + device (one platform surface)
- **DRM root** — `{app_data}/Arcane Powered/drm/`
- Prefer "platform integration" for the SDK overall; ownership is the default launch check inside `arcane_init`, not a separate integration step
- Never call the portal public key a "game id" in prose — say "public key" / "portal public key"
- Do not imply games must call `check_ownership_offline` in addition to init; that is optional / advanced only

## Style preferences

- Use active voice and second person ("you")
- Keep sentences concise — one idea per sentence
- Use sentence case for headings
- Bold for UI elements: Click **Settings**
- Code formatting for file names, commands, paths, and code references
- Code samples: Rust first, then C for FFI; use Bun in shell examples when showing JS tooling

## Content boundaries

- Document public Rust API and C ABI only — public surface is `arcane_init` / `arcane_sdk_init`, optional `check_ownership_offline` / `arcane_check_ownership_offline`, plus `OwnershipStatus` / `SdkError`
- Do not document internal helpers (paths, device hash, raw JWT verify) as integration APIs
- Do not invent achievements / cloud saves / friends APIs until they exist in `src/`
- Do not document launcher-internal endpoints beyond what the SDK reads from disk
- Keep security-sensitive details accurate to source; do not add speculative bypass guidance
- Keep [`contributing.mdx`](./contributing.mdx) aligned with `.github/workflows` (tag-only release, version bump in PR, merge queue)

## Errors in docs

- [`concepts/errors.mdx`](./concepts/errors.mdx) is the source of truth for codes and fixes
- Every public function page must list **which codes that function can return** and link to Errors (debug must be: this function → this code → this problem)
- Avoid duplicating the full fix table on reference pages — short code/problem table + link is enough
