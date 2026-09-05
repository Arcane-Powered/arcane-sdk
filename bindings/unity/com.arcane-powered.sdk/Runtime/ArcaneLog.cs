// SPDX-License-Identifier: MIT OR Apache-2.0
using System;

namespace ArcanePowered
{
    /// <summary>
    /// Where this package writes. Everything routes through here so the SDK
    /// never logs twice, and so the pieces that parse SDK answers stay free of
    /// any engine reference.
    /// </summary>
    internal static class ArcaneLog
    {
        internal static void Info(string message)
        {
#if UNITY_5_3_OR_NEWER
            UnityEngine.Debug.Log("[Arcane] " + message);
#else
            Console.Out.WriteLine("[Arcane] " + message);
#endif
        }

        internal static void Warning(string message)
        {
#if UNITY_5_3_OR_NEWER
            UnityEngine.Debug.LogWarning("[Arcane] " + message);
#else
            Console.Error.WriteLine("[Arcane] " + message);
#endif
        }

        internal static void Error(string message)
        {
#if UNITY_5_3_OR_NEWER
            UnityEngine.Debug.LogError("[Arcane] " + message);
#else
            Console.Error.WriteLine("[Arcane] " + message);
#endif
        }

        internal static void Exception(Exception exception)
        {
#if UNITY_5_3_OR_NEWER
            UnityEngine.Debug.LogException(exception);
#else
            Console.Error.WriteLine(exception);
#endif
        }
    }
}
