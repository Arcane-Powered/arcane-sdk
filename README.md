# arcane-sdk

Native SDK for integrating Arcane Powered — ownership, achievements, cloud saves, friends. Rust core, multi-engine bindings.

```rust
use arcane_sdk::ArcaneClient;

// Once, at launch. Building the client is the ownership check, and it opens
// the play session that measures playtime.
let client = ArcaneClient::init("pk_your_portal_key")?;

client.user_id();   // signed-in Arcane account
client.game_id();   // canonical title id
client.is_owned();  // ownership as of the last check

client.frame();     // once per rendered frame — an atomic load, for FPS sampling
client.session();   // tracking state, playtime, FPS samples

client.achievements().unlock("first_blood")?;   // idempotent, one loopback call
client.friends().list()?;                       // friends, with online and in_game

client.shutdown();  // ends the session and reports the final playtime
```

One background thread (`arcane-session`) reports playtime once a minute and
samples FPS in 30-second windows while the player allows it. Ownership itself is
never revalidated on its own.

Native engines get the same client as a C ABI singleton — see [`include/arcane_sdk.h`](./include/arcane_sdk.h).

The desktop-app side of the wire (endpoints, `session.json`, error bodies) is specified in [`DESKTOP_CONTRACT.md`](./DESKTOP_CONTRACT.md).

Docs live in [`documentation/`](./documentation). Preview with `bunx mint dev` from that folder. Contributor workflow (PRs, SemVer, merge queue, releases): [`documentation/contributing.mdx`](./documentation/contributing.mdx).

Production Ready Documentation can be accessed here [Arcane Powered SDK Documentation](https://docs.arcane-powered.com/sdk)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

## Contribute

1. Open a PR against `main` (merge queue only — no direct pushes).
2. Use a Conventional Commits **PR title**.
3. If the title is `feat` / `fix` / `perf` / breaking, bump **`Cargo.toml` in the PR** (helper below). Otherwise leave the version alone.
4. Keep the branch **up to date** with `main`. After another releasing PR merges, rebase and bump again so SemVer stays correct.
5. On merge of a releasing PR, CI tags `vX.Y.Z`, publishes to crates.io, and creates a GitHub Release. CI never pushes commits to `main`.

| PR title | SemVer in the PR |
|---|---|
| `feat: ...` / `feat(scope): ...` | minor |
| `fix: ...` / `perf: ...` | patch |
| `feat!: ...` / `fix(api)!: ...` | major — but while on `0.x`, breaking bumps the **minor** (`0.3.2` → `0.4.0`), per [SemVer §4](https://semver.org/#spec-item-4) |
| `chore:` / `docs:` / `ci:` / `test:` / `refactor:` / `build:` / `revert:` | unchanged |

```bash
.github/scripts/bump-version.sh "feat: add ownership ticket verification"
```

### C header

```bash
cargo install cbindgen --version 0.29.4 --locked
.github/scripts/generate-header.sh   # after changing src/ffi.rs
```

### Maintainer repo settings (once)

1. **Ruleset on `main`:** PR + merge queue, require branch up to date, optional signed commits. Required checks: `PR title / Validate conventional title`, `PR title / Validate version bump`, `CI / Check + C header`.
2. **Tags:** allow Actions (or `RELEASE_TOKEN`) to create `v*` tags — do not allow bot commits onto `main`.
3. **Secrets:** `CARGO_REGISTRY_TOKEN` (crates.io); optional `RELEASE_TOKEN` for tags/releases if needed.
