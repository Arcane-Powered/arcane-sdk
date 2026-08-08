# Arcane SDK docs

Mintlify documentation for the Arcane SDK — the native bridge between games and Arcane Powered.

## Preview locally

From this directory:

```bash
bunx mint dev
```

Opens at [http://localhost:3000](http://localhost:3000) by default.

## Validate

```bash
bunx mint validate
bunx mint broken-links
```

## Structure

- `docs.json` — site config and navigation
- `index.mdx` / `quickstart.mdx` — getting started (game integration)
- `contributing.mdx` — how the GitHub repo works (PRs, SemVer, merge queue, releases)
- `concepts/` — ownership model and errors
- `reference/` — Rust API and C ABI
