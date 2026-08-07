# arcane-sdk

Native SDK for integrating Arcane Powered — ownership, achievements, cloud saves, friends. Rust core, multi-engine bindings.

Docs live in [`documentation/`](./documentation). Preview with `bunx mint dev` from that folder.

## Versioning

Releases are automated on merge to `main` from the **PR title** (Conventional Commits).

| PR title | SemVer bump | Tag |
|---|---|---|
| `feat: ...` / `feat(scope): ...` | minor | `v0.2.0` |
| `fix: ...` / `perf: ...` | patch | `v0.1.1` |
| `feat!: ...` / `fix(api)!: ...` | major | `v1.0.0` |
| `chore:` / `docs:` / `ci:` / `test:` / `refactor:` / `build:` / `revert:` | none | — |

PR titles are validated by CI. Examples:

```text
feat: add ownership ticket verification
fix(drm): reject expired tickets
feat!: redesign ticket claim API
chore: bump dependencies
```

On a releasing merge, CI bumps `Cargo.toml`, commits `chore(release): vX.Y.Z`, pushes tag `vX.Y.Z`, and creates a GitHub Release.

### Repo settings (required once)

1. **Branch protection** on `main`: require the `PR title / Validate conventional title` check.
2. If `main` blocks direct pushes, either allow GitHub Actions to bypass, or add a `RELEASE_TOKEN` secret (PAT / GitHub App) with permission to push commits + tags to `main`.
