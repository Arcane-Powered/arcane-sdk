// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Runtime.InteropServices;

namespace ArcanePowered.Native
{
    /// <summary>
    /// Sets environment variables where the <em>native</em> side can see them.
    /// </summary>
    /// <remarks>
    /// The SDK reads <c>ARCANE_GAME_ID</c> and <c>ARCANE_USER_ID</c> from the
    /// process environment, which Arcane Powered fills in when it launches the
    /// game. Nothing launches the Unity Editor that way, so the editor
    /// integration sets them itself — and a managed
    /// <see cref="Environment.SetEnvironmentVariable(string,string)"/> is not
    /// enough, because depending on the scripting runtime it may only update a
    /// dictionary the CLR keeps, which Rust's <c>std::env</c> never reads. So we
    /// set both: the process environment through the platform's own call, and
    /// the managed copy so C# agrees with it.
    ///
    /// This is an Editor and local-development convenience. A shipped build gets
    /// the variables from the launcher.
    /// </remarks>
    internal static class ArcaneEnvironment
    {
        [DllImport("kernel32", CharSet = CharSet.Unicode, EntryPoint = "SetEnvironmentVariableW", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool SetEnvironmentVariableWindows(string name, string value);

        [DllImport("libc", EntryPoint = "setenv", SetLastError = true)]
        private static extern int SetEnvironmentVariableLibc(
            [MarshalAs(UnmanagedType.LPStr)] string name,
            [MarshalAs(UnmanagedType.LPStr)] string value,
            int overwrite);

        [DllImport("libSystem.dylib", EntryPoint = "setenv", SetLastError = true)]
        private static extern int SetEnvironmentVariableLibSystem(
            [MarshalAs(UnmanagedType.LPStr)] string name,
            [MarshalAs(UnmanagedType.LPStr)] string value,
            int overwrite);

        /// <summary>
        /// Put <paramref name="name"/> in this process's environment, for native
        /// code as well as managed.
        /// </summary>
        /// <returns>
        /// <see langword="false"/> when the platform call could not be made — the
        /// managed copy is still set, so a caller can carry on and let the SDK
        /// report what it finds.
        /// </returns>
        internal static bool SetProcessVariable(string name, string value)
        {
            if (string.IsNullOrEmpty(name))
            {
                return false;
            }

            string text = value ?? string.Empty;
            Environment.SetEnvironmentVariable(name, text);

            try
            {
                if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
                {
                    return SetEnvironmentVariableWindows(name, text);
                }

                return SetEnvironmentVariableLibc(name, text, 1) == 0;
            }
            catch (DllNotFoundException)
            {
                return SetWithLibSystem(name, text);
            }
            catch (EntryPointNotFoundException)
            {
                return SetWithLibSystem(name, text);
            }
        }

        /// <summary>Fall back to libSystem, which is where macOS keeps <c>setenv</c>.</summary>
        private static bool SetWithLibSystem(string name, string value)
        {
            try
            {
                return SetEnvironmentVariableLibSystem(name, value, 1) == 0;
            }
            catch (DllNotFoundException)
            {
                return false;
            }
            catch (EntryPointNotFoundException)
            {
                return false;
            }
        }
    }
}
