# Arcane Powered SDK for Unity

A UPM package (`com.arcane-powered.sdk`) that wraps the native SDK's
[C ABI](../../include/arcane_sdk.h) as a static C# API, plus the native plugin
your project loads it from.

```csharp
using ArcanePowered;

// Nothing to call at boot: the runtime has already checked ownership before your
// first scene loaded. What is left is deciding what a player who may not play sees.
void Start()
{
    ArcaneError error = ArcaneRuntime.InitializationError;
    if (error != null && error.Code == ArcaneErrorCode.NotOwned)
    {
        ShowStorePage();
        return;
    }

    Arcane.Achievements.Unlock("first_blood");          // idempotent
    ArcaneFriendList friends = Arcane.Friends.List();   // one loopback call
}
```

Requires **Unity 2021.3 or newer**, Mono or IL2CPP, on Windows, macOS or Linux —
the platforms the Arcane Powered desktop app runs on.

## Install

**1. Add the package.** In `Packages/manifest.json`:

```json
{
  "dependencies": {
    "com.arcane-powered.sdk": "https://github.com/Arcane-Powered/arcane-sdk.git?path=bindings/unity/com.arcane-powered.sdk"
  }
}
```

Or copy `com.arcane-powered.sdk/` into your project's `Packages/` folder to be
able to edit it.

**2. Build the native plugin** into that project:

```bash
git clone https://github.com/Arcane-Powered/arcane-sdk
cd arcane-sdk
bindings/unity/build-plugins.sh ~/games/my-game
```

It writes `Assets/Plugins/Arcane/<platform>/` and the package's importer points
each binary at the platform it was built for. Build the platform you are working
on first — that is the one the Editor loads — and add the others before you ship:

```bash
bindings/unity/build-plugins.sh ~/games/my-game --target x86_64-pc-windows-msvc
bindings/unity/build-plugins.sh ~/games/my-game \
    --target aarch64-apple-darwin --target x86_64-apple-darwin   # one universal binary
```

A native library is loaded once per Editor session, so **restart the Editor**
after the first import.

**3. Set the game id for the Editor.** Arcane Powered sets `ARCANE_GAME_ID` and
`ARCANE_USER_ID` on the game process when a player launches from their library.
Nothing launches the Editor that way, so fill in the game id from the portal
under **Project Settings ▸ Arcane Powered**; the package puts it in the process
environment before it initialises. A variable already set in the environment that
started the Editor wins, so a launch profile still decides.

You also need the **Arcane desktop app running and signed in**, with an account
that owns the title — see
[Local development](https://docs.arcane-powered.com/sdk/concepts/local-development).

## What runs by itself

`ArcaneRuntime` creates itself before the first scene loads and, by default:

- calls `Arcane.Init()` — the ownership check — and leaves the result in
  `ArcaneRuntime.InitializationError`;
- calls `Arcane.Frame()` every frame, which is what FPS sampling counts;
- reports the resolution and quality preset, and again whenever they change;
- drains the lobby event queue once a second and raises the `Arcane.Lobbies`
  events on the main thread;
- calls `Arcane.Shutdown()` on quit, and when you leave play mode in the Editor —
  the native library outlives a play-mode run, so a client left behind would keep
  reporting playtime for a game nobody is playing.

Each of those is a checkbox in **Project Settings ▸ Arcane Powered**. Turn one
off and do it yourself; the static API is the same either way.

## The API

Everything hangs off `Arcane`:

| | |
|---|---|
| `Arcane.TryInit(out error)` / `Init()` | Check ownership and build the client |
| `Arcane.IsOwned`, `Ownership` | Ownership as of the last check |
| `Arcane.UserId`, `GameId`, `DeviceHash` | Who and what this client is |
| `Arcane.Session` | Playtime, FPS samples, what the background thread is doing |
| `Arcane.Refresh()`, `Shutdown()`, `Frame()`, `SetGraphics(…)` | Lifecycle |
| `Arcane.Achievements` | `Unlock`, `List`, `IsUnlocked` |
| `Arcane.Friends` | `List` — with `Online`, `InGame` and `Stale` |
| `Arcane.Lobbies` | `Create`, `Join`, `JoinByCode`, `Get`, `Invite`, `Leave`, `Close`, `PollEvents`, `LaunchJoinCode` |
| `Arcane.LastError` | The last failure, for a debug overlay or a crash report |

Three conventions run through all of it:

**Failures are values.** Every call that can fail has a `TryX(out ArcaneError)`
form that returns `false`, and an `X()` form that throws `ArcaneException`. The
error carries a `Code` to branch on, a `Message` safe to show a player, and a
`Hint` and `Context` for your logs. A code added to the SDK after this package
was built reads as `ArcaneErrorCode.Unknown` with its wire string in `CodeName`.

**Blocking calls say so.** `Unlock`, `List`, and every lobby call make one
synchronous loopback call to the desktop app — fine on a loading screen, not in
`Update`. Each has an `…Async` twin that runs it on a background thread:

```csharp
ArcaneFriendList friends = await Arcane.Friends.ListAsync();
Arcane.RunOnMainThread(() => PopulateFriendsMenu(friends));   // back to the scene
```

**Bytes stay bytes.** Lobby payloads are `byte[]` — the base64 the C ABI wants
never reaches your code.

## Lobbies

Arcane holds the lobby, the membership and the join code, and hands every player
the connection blobs of the others. Connecting to them is your netcode's job.

```csharp
ArcaneLobby lobby = Arcane.Lobbies.Create(4, ArcaneLobbyVisibility.FriendsAndCode, MyEndpoint());
ShowJoinCode(lobby.JoinCode);

Arcane.Lobbies.MemberJoined += e => ConnectTo(e.Payload);   // main thread
Arcane.Lobbies.ResyncRequested += e => Refresh();           // ask, don't replay
```

`Arcane.Lobbies.LaunchJoinCode` is set when the player started the game from a
friend's **Join** in the launcher — check it once at boot.

## Samples and tests

The package ships two samples, importable from the Package Manager: **Quick
start** (boot, ownership, achievements) and **Lobbies** (host, invite, join,
events). Its tests run in the Unity Test Runner under **EditMode** and cover the
JSON reader, the model mapping and the buffer growth loop against the documents
the C ABI actually writes.

## Troubleshooting

| What you see | What it means |
|---|---|
| `ArcaneErrorCode.PluginMissing`, or "Native plugin: not loaded" in Project Settings | The binary is not in the project, or the Editor was not restarted after importing it |
| `missing_game_id` | Nothing set `ARCANE_GAME_ID` — fill in the Editor game id in Project Settings |
| `arcane_unavailable` | The Arcane desktop app is not running |
| `not_owned` | The signed-in account does not own the title. Grant it in the portal, or disable DRM on the title |
| `feature_unavailable` | The desktop app predates the route — update it |

**Tools ▸ Arcane Powered ▸ Log diagnostics** writes what the SDK can see right
now to the console; it is the fastest thing to attach to a bug report.

Full reference: [docs.arcane-powered.com/sdk](https://docs.arcane-powered.com/sdk).
