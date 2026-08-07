> For Mintlify product knowledge (components, configuration, writing standards),
> install the Mintlify skill: `bunx skills add https://mintlify.com/docs`

# Documentation project instructions

## About this project

- Docs for **Arcane SDK** (`arcane-sdk`), the Rust core for Arcane Powered game integrations
- Site root is this `documentation/` directory (`docs.json` here)
- Preview locally with: `bunx mint dev` (from this directory)
- Prefer Bun over npm/pnpm for CLI tooling in this repo

## Terminology

- **Arcane Powered** — the platform / desktop launcher
- **Arcane SDK** — this native crate and its C ABI
- **Ownership ticket** — cached JWT proving offline ownership for a game + device
- **DRM root** — `{app_data}/Arcane Powered/drm/`
- Prefer "ownership check" / "offline verification" over vague "auth"
- Prefer `game_id` (not "app id" or "product id") when referring to the SDK argument

## Style preferences

- Use active voice and second person ("you")
- Keep sentences concise — one idea per sentence
- Use sentence case for headings
- Bold for UI elements: Click **Settings**
- Code formatting for file names, commands, paths, and code references
- Code samples: Rust first, then C for FFI; use Bun in shell examples when showing JS tooling

## Content boundaries

- Document public Rust API and C ABI only
- Do not invent achievements / cloud saves / friends APIs until they exist in `src/`
- Do not document launcher-internal endpoints beyond what the SDK reads from disk
- Keep security-sensitive details accurate to source; do not add speculative bypass guidance
