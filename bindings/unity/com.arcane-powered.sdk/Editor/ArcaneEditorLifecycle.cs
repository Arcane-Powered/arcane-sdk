// SPDX-License-Identifier: MIT OR Apache-2.0
using UnityEditor;

namespace ArcanePowered.Editor
{
    /// <summary>
    /// Ends the play session when a play-mode run ends, and before a domain
    /// reload.
    /// </summary>
    /// <remarks>
    /// A build's process dies when the game quits and takes the client with it.
    /// The Editor's does not: the native library stays loaded from one run to the
    /// next, so a client left behind would keep a play session open across runs
    /// and report playtime for a game nobody is playing. Shutting down here
    /// reports the real playtime and releases the singleton, exactly as quitting
    /// a build would.
    /// </remarks>
    [InitializeOnLoad]
    internal static class ArcaneEditorLifecycle
    {
        static ArcaneEditorLifecycle()
        {
            EditorApplication.playModeStateChanged -= OnPlayModeChanged;
            EditorApplication.playModeStateChanged += OnPlayModeChanged;

            // A recompile mid-run throws away every managed object that was
            // tracking the session, so the native side has to let go too.
            AssemblyReloadEvents.beforeAssemblyReload -= OnBeforeAssemblyReload;
            AssemblyReloadEvents.beforeAssemblyReload += OnBeforeAssemblyReload;
        }

        private static void OnPlayModeChanged(PlayModeStateChange change)
        {
            if (change == PlayModeStateChange.ExitingPlayMode)
            {
                Arcane.Shutdown();
            }
        }

        private static void OnBeforeAssemblyReload()
        {
            Arcane.Shutdown();
        }
    }
}
