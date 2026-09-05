// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Runtime.InteropServices;

namespace ArcanePowered.Native
{
    /// <summary>
    /// One-to-one P/Invoke declarations for <c>include/arcane_sdk.h</c>.
    /// </summary>
    /// <remarks>
    /// Nothing here interprets the SDK: the return codes are the ones the C ABI
    /// documents, strings cross the boundary as NUL-terminated UTF-8 byte
    /// arrays — never as marshalled <see cref="string"/>, whose default ANSI
    /// encoding would mangle any non-ASCII pseudo — and every buffer is
    /// allocated by the caller. The wrapper around it lives in
    /// <see cref="Arcane"/>.
    /// </remarks>
    internal static class ArcaneNative
    {
#if (UNITY_IOS || UNITY_TVOS || UNITY_VISIONOS || UNITY_WEBGL) && !UNITY_EDITOR
        /// <summary>Statically linked platforms resolve the symbols in the executable itself.</summary>
        internal const string Lib = "__Internal";
#else
        /// <summary>
        /// Unity resolves this to <c>arcane_sdk.dll</c>, <c>libarcane_sdk.so</c>
        /// or <c>arcane_sdk.bundle</c> depending on the platform.
        /// </summary>
        internal const string Lib = "arcane_sdk";
#endif

        // Action return codes.
        internal const int Ok = 0;
        internal const int ErrArgument = 1;
        internal const int ErrSdk = 2;

        // Getter return codes (negative).
        internal const int ErrNotInitialized = -1;
        internal const int ErrBadBuffer = -2;
        internal const int ErrBufferTooSmall = -3;
        internal const int ErrUnavailable = -4;

        // arcane_sdk_ownership results.
        internal const int OwnershipOwned = 0;
        internal const int OwnershipDrmDisabled = 1;

        // arcane_sdk_lobby_create visibility values.
        internal const int LobbyFriends = 0;
        internal const int LobbyCode = 1;
        internal const int LobbyFriendsAndCode = 2;

        private const CallingConvention Cdecl = CallingConvention.Cdecl;

        // --- Lifecycle -----------------------------------------------------

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_init(byte[] err_buf, UIntPtr err_len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_refresh(byte[] err_buf, UIntPtr err_len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern void arcane_sdk_shutdown();

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_is_initialized();

        // --- Session -------------------------------------------------------

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern void arcane_sdk_frame();

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_set_graphics(byte[] resolution, byte[] preset);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_session_json(byte[] buf, UIntPtr len);

        // --- Ownership -----------------------------------------------------

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_ownership();

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern long arcane_sdk_ticket_expires_at();

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern long arcane_sdk_checked_at();

        // --- Identity ------------------------------------------------------

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_user_id(byte[] buf, UIntPtr len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_game_id(byte[] buf, UIntPtr len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_device_hash(byte[] buf, UIntPtr len);

        // --- Achievements --------------------------------------------------

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_achievements_json(byte[] buf, UIntPtr len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_achievement_unlock(byte[] key, byte[] err_buf, UIntPtr err_len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_achievement_is_unlocked(byte[] key);

        // --- Friends -------------------------------------------------------

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_friends_json(byte[] buf, UIntPtr len);

        // --- Lobbies -------------------------------------------------------

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_lobby_create(
            byte max_players,
            int visibility,
            byte[] payload_b64,
            byte[] buf,
            UIntPtr len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_lobby_join(byte[] lobby_id, byte[] payload_b64, byte[] buf, UIntPtr len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_lobby_join_code(byte[] join_code, byte[] payload_b64, byte[] buf, UIntPtr len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_lobby_get(byte[] lobby_id, byte[] buf, UIntPtr len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_lobby_invite(byte[] lobby_id, byte[] to_user_id, byte[] err_buf, UIntPtr err_len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_lobby_leave(byte[] lobby_id, byte[] err_buf, UIntPtr err_len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_lobby_close(byte[] lobby_id, byte[] err_buf, UIntPtr err_len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_lobby_events_json(byte[] buf, UIntPtr len);

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_launch_join_code(byte[] buf, UIntPtr len);

        // --- Diagnostics ---------------------------------------------------

        [DllImport(Lib, CallingConvention = Cdecl)]
        internal static extern int arcane_sdk_last_error_json(byte[] buf, UIntPtr len);

        // --- Availability --------------------------------------------------

        private static bool _probed;
        private static bool _available;

        /// <summary>
        /// Whether the native library could be loaded. A missing plugin is a
        /// build or install problem, not an SDK error, so every entry point
        /// checks this first and reports <c>plugin_missing</c> rather than
        /// letting a <see cref="DllNotFoundException"/> escape into gameplay
        /// code.
        /// </summary>
        internal static bool IsAvailable
        {
            get
            {
                if (_probed)
                {
                    return _available;
                }

                try
                {
                    arcane_sdk_is_initialized();
                    _available = true;
                }
                catch (DllNotFoundException)
                {
                    _available = false;
                }
                catch (EntryPointNotFoundException)
                {
                    _available = false;
                }

                _probed = true;
                return _available;
            }
        }
    }
}
