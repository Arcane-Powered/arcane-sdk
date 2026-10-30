# Engine bindings

The Rust crate is the SDK; everything here is a thin, idiomatic wrapper over the
same [C ABI](../include/arcane_sdk.h) so a game does not have to write P/Invoke,
GDExtension glue or `FPlatformProcess::GetDllExport` by hand.

| Engine | Status | Where |
|--------|--------|-------|
| Unity  | Available | [`unity/`](./unity) — UPM package `com.arcane-powered.sdk` |
| Web (HTML5) | Available | [`js/`](./js) — npm package `@arcane/sdk-web` |
| Godot  | Planned   | — |
| Unreal | Planned   | — |

Every binding follows the same shape, so a team that ships on two engines reads
one set of docs:

- **The client is a process-wide singleton.** No handle to carry through the
  engine's scripting layer: initialise once at launch, then call the static API.
- **The names match the Rust API.** `init`, `refresh`, `frame`, `shutdown`,
  `achievements`, `friends`, lobbies — spelled the way the host language spells
  things, but never renamed.
- **Failures are values, not exceptions**, with the exception-throwing twin
  offered where the language expects one. The [error
  codes](https://docs.arcane-powered.com/sdk/concepts/errors) are the ones the
  crate documents.
- **The engine's own lifecycle is handled for you**: frames counted, the play
  session ended on quit, and the editor's play-mode reload treated as a quit —
  which matters, because an editor keeps the native library loaded between runs.
- **Blocking calls are marked as blocking.** Anything that talks to the Arcane
  desktop app is one synchronous loopback call and never belongs in a render
  loop; each binding offers a background-thread form of it.

Adding a binding for another engine means wrapping the same twenty-odd C
functions. Start from [`unity/`](./unity): the layering there — raw P/Invoke, a
buffer/marshalling layer, a JSON reader, typed models, a static façade, an
engine-lifecycle component — is the same layering the others want.

The web binding is the one exception to that layering: a browser game cannot load the
native library, so [`js/`](./js) speaks `postMessage` to the desktop's play window
instead of the C ABI, keeping the same names. It also departs from the shape above:
`Arcane.init()` resolves to an instance the game keeps rather than a singleton, every call
is asynchronous, a refused call rejects its promise with an `ArcaneError`, and the codes are
the desktop app's (`offline` where the native SDK says `network_required`).
