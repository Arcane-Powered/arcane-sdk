# Arcane Powered SDK

Ownership, playtime, achievements, friends and lobbies from
[Arcane Powered](https://arcane-powered.com), as a static C# API over the native
SDK.

```csharp
using ArcanePowered;

void Start()
{
    // The ownership check already ran, before this scene loaded.
    ArcaneError error = ArcaneRuntime.InitializationError;
    if (error != null && error.Code == ArcaneErrorCode.NotOwned)
    {
        ShowStorePage();
        return;
    }

    Arcane.Achievements.Unlock("first_blood");   // idempotent
}
```

## Setup

1. **Build the native plugin** into this project — the package is C# only:

   ```bash
   bindings/unity/build-plugins.sh /path/to/this/project
   ```

   from a clone of [arcane-sdk](https://github.com/Arcane-Powered/arcane-sdk).
   Then restart the Editor: a native library is loaded once per session.

2. **Set the Editor game id** in **Project Settings ▸ Arcane Powered**. A shipped
   build gets it from the launcher; nothing sets it for the Editor.

3. Run the **Arcane Powered desktop app**, signed in to an account that owns the
   title.

## What runs by itself

`ArcaneRuntime` initialises before the first scene, counts frames, reports the
resolution and quality preset, pumps lobby events onto the main thread, and ends
the play session on quit — and when you leave play mode, which the Editor
otherwise treats as a game that never stopped. Every one of those is a checkbox
in Project Settings.

## Where to look next

- **Samples** — *Quick start* and *Lobbies*, importable from the Package Manager.
- **Full documentation** — [docs.arcane-powered.com/sdk](https://docs.arcane-powered.com/sdk).
- **Diagnostics** — **Tools ▸ Arcane Powered ▸ Log diagnostics**.
