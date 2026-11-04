# @arcane/sdk-web

Achievements, friends, lobbies and cloud-synced saves for an HTML5 game played in the
Arcane Powered desktop app.

A browser game has no native library and no loopback port: it runs in an iframe of the
desktop's play window, and this package talks to that window with `postMessage`. The
desktop checks every call (origin, method allow-list, rate limit) and answers with the
same data the native SDK gets.

```ts
import { Arcane } from "@arcane/sdk-web";

const arcane = await Arcane.init();
if (arcane.available) {
  arcane.on("pause", () => game.pause());
  arcane.on("resume", () => game.resume());
  await arcane.achievements.unlock("first-blood");
  await arcane.saves.write("slot-1", JSON.stringify(state));
}
```

The build has to declare the bridge when it is published from the publisher portal
(**The game uses the Arcane web SDK**). Without it, or outside the app (your own browser, an itch
page), `init()` resolves with `available: false` after one second and every call
resolves to an empty value, so the same build runs everywhere.

Full guide: <https://docs.arcane-powered.com/sdk/engines/web>.

## License

MIT OR Apache-2.0.
