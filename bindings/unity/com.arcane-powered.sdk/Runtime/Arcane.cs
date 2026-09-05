// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using ArcanePowered.Json;
using ArcanePowered.Native;

namespace ArcanePowered
{
    /// <summary>
    /// The Arcane Powered SDK, as one static entry point.
    /// </summary>
    /// <remarks>
    /// The client is a process-wide singleton: <see cref="Init()"/> once at
    /// launch, then read from the getters — there is no handle to carry around
    /// and no instance to inject. Building it <em>is</em> the ownership check,
    /// and it opens the play session that measures playtime.
    ///
    /// You pass no ids: Arcane Powered sets <c>ARCANE_GAME_ID</c> and
    /// <c>ARCANE_USER_ID</c> on the game process when it launches it. In the
    /// Editor, where nothing launches your game, fill in
    /// <see cref="ArcaneSettings"/> instead.
    ///
    /// With <see cref="ArcaneSettings.AutoInitialize"/> on — the default —
    /// <see cref="ArcaneRuntime"/> calls <see cref="Init()"/> before the first
    /// scene loads, counts frames for you, and shuts down on quit. Turn it off
    /// to drive the lifecycle yourself.
    /// </remarks>
    public static class Arcane
    {
        private static readonly ArcaneAchievements AchievementsApi = new ArcaneAchievements();
        private static readonly ArcaneFriends FriendsApi = new ArcaneFriends();
        private static readonly ArcaneLobbies LobbiesApi = new ArcaneLobbies();
        private static readonly Queue<Action> MainThreadWork = new Queue<Action>();

        /// <summary>
        /// Whether the native plugin could be loaded. <see langword="false"/>
        /// means the library is missing from the project, not that anything went
        /// wrong with Arcane.
        /// </summary>
        public static bool IsPluginAvailable
        {
            get { return ArcaneNative.IsAvailable; }
        }

        /// <summary>Whether a client is currently initialised.</summary>
        public static bool IsInitialized
        {
            get { return ArcaneNative.IsAvailable && ArcaneNative.arcane_sdk_is_initialized() == 1; }
        }

        /// <summary>Achievements for the signed-in player.</summary>
        public static ArcaneAchievements Achievements
        {
            get { return AchievementsApi; }
        }

        /// <summary>This player's friends, and their presence.</summary>
        public static ArcaneFriends Friends
        {
            get { return FriendsApi; }
        }

        /// <summary>Lobbies and invitations.</summary>
        public static ArcaneLobbies Lobbies
        {
            get { return LobbiesApi; }
        }

        // --- Lifecycle -----------------------------------------------------

        /// <summary>
        /// Verify ownership and build the client, answering
        /// <see langword="false"/> instead of throwing.
        /// </summary>
        /// <remarks>
        /// This is the ownership check, so <see cref="ArcaneErrorCode.NotOwned"/>
        /// is a normal outcome, not an exceptional one — which is why the boot
        /// path is written against this overload rather than
        /// <see cref="Init()"/>. It makes a synchronous loopback call and can
        /// take a moment when the desktop app has to be launched first.
        /// </remarks>
        public static bool TryInit(out ArcaneError error)
        {
            return ArcaneCall.Run(ArcaneNative.arcane_sdk_init, out error);
        }

        /// <summary>
        /// Verify ownership and build the client, throwing
        /// <see cref="ArcaneException"/> when the player may not play.
        /// </summary>
        public static void Init()
        {
            ArcaneError error;
            if (!TryInit(out error))
            {
                throw new ArcaneException(error);
            }
        }

        /// <summary>
        /// End the play session, reporting the final playtime, and drop the
        /// client.
        /// </summary>
        /// <remarks>
        /// Safe to call when no client exists. <see cref="ArcaneRuntime"/> calls
        /// it on quit, and the editor integration calls it when you leave play
        /// mode — the native library stays loaded between runs, so a client left
        /// behind would outlive the run that built it.
        /// </remarks>
        public static void Shutdown()
        {
            if (!ArcaneNative.IsAvailable)
            {
                return;
            }

            ArcaneNative.arcane_sdk_shutdown();
        }

        /// <summary>
        /// Re-run the ownership check and update the client, answering
        /// <see langword="false"/> instead of throwing. On failure the client
        /// keeps its previous state.
        /// </summary>
        public static bool TryRefresh(out ArcaneError error)
        {
            return ArcaneCall.Run(ArcaneNative.arcane_sdk_refresh, out error);
        }

        /// <summary>Re-run the ownership check, throwing on failure.</summary>
        public static void Refresh()
        {
            ArcaneError error;
            if (!TryRefresh(out error))
            {
                throw new ArcaneException(error);
            }
        }

        /// <summary>Re-run the ownership check on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers <see langword="false"/> and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public static Task<bool> RefreshAsync()
        {
            return Task.Run(() =>
            {
                ArcaneError ignored;
                return TryRefresh(out ignored);
            });
        }

        // --- Session -------------------------------------------------------

        /// <summary>
        /// Count one rendered frame, for FPS sampling.
        /// </summary>
        /// <remarks>
        /// One relaxed atomic operation — this is the one entry point meant for
        /// the render loop. <see cref="ArcaneRuntime"/> calls it every frame
        /// while <see cref="ArcaneSettings.CountFrames"/> is on; call it
        /// yourself only if you turned that off.
        /// </remarks>
        public static void Frame()
        {
            if (!ArcaneNative.IsAvailable)
            {
                return;
            }

            ArcaneNative.arcane_sdk_frame();
        }

        /// <summary>
        /// Record the current display settings, attached to the FPS samples that
        /// follow — for example <c>"2560x1440"</c> and <c>"high"</c>.
        /// </summary>
        /// <remarks>
        /// Empty strings clear the values. This takes a short lock, so call it
        /// when the player changes a setting, never per frame.
        /// </remarks>
        public static bool TrySetGraphics(string resolution, string preset, out ArcaneError error)
        {
            if (!ArcaneNative.IsAvailable)
            {
                error = ArcaneError.PluginMissing();
                return false;
            }

            byte[] encodedResolution = ArcaneBuffer.ToUtf8(resolution ?? string.Empty);
            byte[] encodedPreset = ArcaneBuffer.ToUtf8(preset ?? string.Empty);

            if (ArcaneNative.arcane_sdk_set_graphics(encodedResolution, encodedPreset) == ArcaneNative.Ok)
            {
                error = null;
                return true;
            }

            error = IsInitialized
                ? ArcaneError.Argument("The Arcane SDK rejected the graphics settings.", null)
                : ArcaneError.NotInitialized();
            return false;
        }

        /// <summary>Record the current display settings, throwing on failure.</summary>
        public static void SetGraphics(string resolution, string preset)
        {
            ArcaneError error;
            if (!TrySetGraphics(resolution, preset, out error))
            {
                throw new ArcaneException(error);
            }
        }

        /// <summary>
        /// The play session: how long this run has been measured, and what the
        /// background thread is doing.
        /// </summary>
        /// <remarks>Reads memory only — cheap enough for a debug overlay.</remarks>
        public static ArcaneSessionInfo Session
        {
            get
            {
                if (!ArcaneNative.IsAvailable)
                {
                    return ArcaneSessionInfo.Empty();
                }

                string json;
                if (ArcaneBuffer.Read(
                        ArcaneNative.arcane_sdk_session_json,
                        ArcaneBuffer.SessionBufferSize,
                        out json) < 0)
                {
                    return ArcaneSessionInfo.Empty();
                }

                ArcaneJson root;
                return ArcaneJson.TryParse(json, out root)
                    ? ArcaneSessionInfo.FromJson(root)
                    : ArcaneSessionInfo.Empty();
            }
        }

        // --- Ownership -----------------------------------------------------

        /// <summary>Ownership as of the last check.</summary>
        public static ArcaneOwnership Ownership
        {
            get
            {
                if (!ArcaneNative.IsAvailable)
                {
                    return ArcaneOwnership.NotInitialized;
                }

                switch (ArcaneNative.arcane_sdk_ownership())
                {
                    case ArcaneNative.OwnershipOwned: return ArcaneOwnership.Owned;
                    case ArcaneNative.OwnershipDrmDisabled: return ArcaneOwnership.DrmDisabled;
                    default: return ArcaneOwnership.NotInitialized;
                }
            }
        }

        /// <summary>
        /// Whether this player may play: a valid ticket, or a title with DRM
        /// disabled.
        /// </summary>
        public static bool IsOwned
        {
            get
            {
                ArcaneOwnership ownership = Ownership;
                return ownership == ArcaneOwnership.Owned || ownership == ArcaneOwnership.DrmDisabled;
            }
        }

        /// <summary>When the ownership ticket expires, or <see langword="null"/> when unknown.</summary>
        public static DateTimeOffset? TicketExpiresAt
        {
            get { return FromUnix(ArcaneNative.IsAvailable ? ArcaneNative.arcane_sdk_ticket_expires_at() : -1); }
        }

        /// <summary>When ownership was last checked, or <see langword="null"/> when unknown.</summary>
        public static DateTimeOffset? CheckedAt
        {
            get { return FromUnix(ArcaneNative.IsAvailable ? ArcaneNative.arcane_sdk_checked_at() : -1); }
        }

        // --- Identity ------------------------------------------------------

        /// <summary>The signed-in Arcane account, or <see langword="null"/> before init.</summary>
        public static string UserId
        {
            get { return ArcaneCall.ReadValue(ArcaneNative.arcane_sdk_user_id, ArcaneBuffer.IdBufferSize); }
        }

        /// <summary>The game id this client was initialised with, or <see langword="null"/> before init.</summary>
        public static string GameId
        {
            get { return ArcaneCall.ReadValue(ArcaneNative.arcane_sdk_game_id, ArcaneBuffer.IdBufferSize); }
        }

        /// <summary>This machine's device fingerprint, or <see langword="null"/> before init.</summary>
        public static string DeviceHash
        {
            get { return ArcaneCall.ReadValue(ArcaneNative.arcane_sdk_device_hash, ArcaneBuffer.IdBufferSize); }
        }

        // --- Diagnostics ---------------------------------------------------

        /// <summary>
        /// The last failure the SDK recorded, or <see langword="null"/> when
        /// nothing has failed since the last success.
        /// </summary>
        /// <remarks>
        /// The methods here hand you the error directly; this is for the times
        /// you want it later — a debug overlay, a crash report.
        /// </remarks>
        public static ArcaneError LastError
        {
            get { return ArcaneError.FromLastError(); }
        }

        // --- Main-thread dispatch ------------------------------------------

        /// <summary>
        /// Run <paramref name="work"/> on Unity's main thread, on the next
        /// frame.
        /// </summary>
        /// <remarks>
        /// The <c>…Async</c> methods complete on a background thread, where
        /// touching the scene is illegal; hand the result back through here.
        /// Without <see cref="ArcaneRuntime"/> in the scene there is no frame to
        /// wait for, so the work runs inline instead of being dropped.
        /// </remarks>
        public static void RunOnMainThread(Action work)
        {
            if (work == null)
            {
                return;
            }

            if (!MainThreadPumpActive)
            {
                work();
                return;
            }

            lock (MainThreadWork)
            {
                MainThreadWork.Enqueue(work);
            }
        }

        /// <summary>
        /// Whether a <see cref="ArcaneRuntime"/> is alive to drain the queue.
        /// Without one, <see cref="RunOnMainThread"/> has no frame to wait for.
        /// </summary>
        internal static bool MainThreadPumpActive { get; set; }

        /// <summary>Run everything queued by <see cref="RunOnMainThread"/>. Called once per frame.</summary>
        internal static void DrainMainThreadWork()
        {
            while (true)
            {
                Action work;
                lock (MainThreadWork)
                {
                    if (MainThreadWork.Count == 0)
                    {
                        return;
                    }

                    work = MainThreadWork.Dequeue();
                }

                try
                {
                    work();
                }
                catch (Exception exception)
                {
                    ArcaneLog.Exception(exception);
                }
            }
        }

        private static DateTimeOffset? FromUnix(long seconds)
        {
            return seconds < 0 ? (DateTimeOffset?)null : DateTimeOffset.FromUnixTimeSeconds(seconds);
        }
    }
}
