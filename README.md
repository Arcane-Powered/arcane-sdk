# arcane-sdk

Native SDK for integrating Arcane Powered — ownership, achievements, cloud saves, friends. Rust core, multi-engine bindings.

Docs live in [`documentation/`](./documentation). Preview with `bunx mint dev` from that folder.

Production Ready Documentation can be accessed here [Arcane Powered SDK Documentation](https://docs.arcane-powered.com/sdk)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

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

On a releasing merge, CI bumps `Cargo.toml`, publishes to **crates.io**, pushes tag `vX.Y.Z`, and creates a GitHub Release (with `include/arcane_sdk.h` attached).

### Repo settings (required once)

1. **Branch protection** on `main`: require the `PR title / Validate conventional title` check.
2. If `main` blocks direct pushes, either allow GitHub Actions to bypass, or add a `RELEASE_TOKEN` secret (PAT / GitHub App) with permission to push commits + tags to `main`.
3. **`CARGO_REGISTRY_TOKEN`** secret: crates.io API token with publish permission for this crate.

### C header

```bash
# Pin matches CI (.github/workflows/ci.yml)
cargo install cbindgen --version 0.29.4 --locked
# Regenerate after changing src/ffi.rs
.github/scripts/generate-header.sh
```

Committed at [`include/arcane_sdk.h`](include/arcane_sdk.h); PRs verify it stays in sync. Engines include it and link the `cdylib` / `staticlib` from `cargo build --release`. The release job never installs cbindgen (keeps publish credentials away from registry-installed tools).
