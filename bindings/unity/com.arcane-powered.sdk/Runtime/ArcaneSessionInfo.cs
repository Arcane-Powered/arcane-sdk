// SPDX-License-Identifier: MIT OR Apache-2.0
using ArcanePowered.Json;

namespace ArcanePowered
{
    /// <summary>Whether the SDK is tracking this play session.</summary>
    public enum ArcaneTracking
    {
        /// <summary>Neither reported nor measured — the player opted out, or nobody is signed in.</summary>
        Disabled = 0,

        /// <summary>Measured locally, but the desktop app has not accepted the session yet.</summary>
        Pending,

        /// <summary>Reported to Arcane once a minute, with FPS sampled in 30-second windows.</summary>
        Active,
    }

    /// <summary>Whether the session thread is polling Arcane for lobby events.</summary>
    public enum ArcaneLobbyPolling
    {
        /// <summary>Nothing is polled: no lobby call has been made, or the play session is off.</summary>
        Off = 0,

        /// <summary>Armed — the session thread asks for events on every tick.</summary>
        Active,

        /// <summary>The Arcane desktop app predates the lobby routes; polling will not restart.</summary>
        Unavailable,
    }

    /// <summary>
    /// A snapshot of the play session: how long this run has been measured, and
    /// what the background thread is doing.
    /// </summary>
    public sealed class ArcaneSessionInfo
    {
        private ArcaneSessionInfo()
        {
        }

        /// <summary>Arcane's id for this run, once the desktop app has accepted it.</summary>
        public string SessionId { get; private set; }

        /// <summary>Whether playtime is being reported, measured only, or neither.</summary>
        public ArcaneTracking Tracking { get; private set; }

        /// <summary>Seconds measured so far in this run.</summary>
        public long PlayedSeconds { get; private set; }

        /// <summary>Whether an FPS sampling window is open right now.</summary>
        public bool FpsSampling { get; private set; }

        /// <summary>How many FPS samples this run has produced.</summary>
        public int SamplesTaken { get; private set; }

        /// <summary>The average of the last closed FPS window, or <see langword="null"/> before the first.</summary>
        public float? LastFpsAverage { get; private set; }

        /// <summary>Whether lobby events are being polled.</summary>
        public ArcaneLobbyPolling LobbyEvents { get; private set; }

        /// <summary>The state a getter reports before a client exists.</summary>
        internal static ArcaneSessionInfo Empty()
        {
            return new ArcaneSessionInfo
            {
                Tracking = ArcaneTracking.Disabled,
                LobbyEvents = ArcaneLobbyPolling.Off,
            };
        }

        /// <summary>Parse the document <c>arcane_sdk_session_json</c> writes.</summary>
        internal static ArcaneSessionInfo FromJson(ArcaneJson root)
        {
            return new ArcaneSessionInfo
            {
                SessionId = root["session_id"].AsString(),
                Tracking = ParseTracking(root["tracking"].AsString()),
                PlayedSeconds = root["played_seconds"].AsLong(),
                FpsSampling = root["fps_sampling"].AsBool(),
                SamplesTaken = (int)root["samples_taken"].AsLong(),
                LastFpsAverage = root["last_fps_avg"].AsNullableFloat(),
                LobbyEvents = ParsePolling(root["lobby_events"].AsString()),
            };
        }

        private static ArcaneTracking ParseTracking(string value)
        {
            switch (value)
            {
                case "active": return ArcaneTracking.Active;
                case "pending": return ArcaneTracking.Pending;
                default: return ArcaneTracking.Disabled;
            }
        }

        private static ArcaneLobbyPolling ParsePolling(string value)
        {
            switch (value)
            {
                case "active": return ArcaneLobbyPolling.Active;
                case "unavailable": return ArcaneLobbyPolling.Unavailable;
                default: return ArcaneLobbyPolling.Off;
            }
        }
    }
}
