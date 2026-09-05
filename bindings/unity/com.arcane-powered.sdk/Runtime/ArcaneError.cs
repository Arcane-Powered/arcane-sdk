// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Collections.Generic;
using System.Text;
using ArcanePowered.Json;
using ArcanePowered.Native;

namespace ArcanePowered
{
    /// <summary>
    /// The stable failure codes of the SDK, plus the two the Unity wrapper
    /// raises on its own.
    /// </summary>
    /// <remarks>
    /// A code the SDK adds after this package was built reads as
    /// <see cref="Unknown"/>; <see cref="ArcaneError.CodeName"/> still carries
    /// the wire string, so logging it is always right.
    /// </remarks>
    public enum ArcaneErrorCode
    {
        /// <summary>A code this package does not know. Read <see cref="ArcaneError.CodeName"/>.</summary>
        Unknown = 0,

        /// <summary>No ticket cached for the signed-in account, or an empty one while DRM is on.</summary>
        TicketMissing,

        /// <summary>The ownership ticket is past its expiry.</summary>
        TicketExpired,

        /// <summary>The ticket is malformed, for another title, or unverifiable.</summary>
        TicketInvalid,

        /// <summary>The ticket is bound to another machine.</summary>
        DeviceMismatch,

        /// <summary>The system clock is earlier than the ticket or the last-seen time.</summary>
        ClockRollback,

        /// <summary>The signed-in account does not own this title.</summary>
        NotOwned,

        /// <summary>No usable offline ticket, and the refresh needs the network.</summary>
        NetworkRequired,

        /// <summary>Nobody is signed in to the Arcane desktop app.</summary>
        NotAuthenticated,

        /// <summary>The Arcane desktop app could not be reached or launched.</summary>
        ArcaneUnavailable,

        /// <summary>The Arcane desktop app predates the route the SDK asked for.</summary>
        FeatureUnavailable,

        /// <summary>The title defines no achievement with that key.</summary>
        UnknownAchievement,

        /// <summary>No open lobby with that id or join code.</summary>
        LobbyNotFound,

        /// <summary>The lobby already holds its maximum number of players.</summary>
        LobbyFull,

        /// <summary>The host closed the lobby, or their play session expired.</summary>
        LobbyClosed,

        /// <summary>The lobby is friends-only and this account is not one of the host's.</summary>
        NotFriends,

        /// <summary><c>ARCANE_GAME_ID</c> is unset — the game was started outside Arcane Powered.</summary>
        MissingGameId,

        /// <summary>The value in <c>ARCANE_GAME_ID</c> is oversized or has a forbidden character.</summary>
        InvalidGameId,

        /// <summary>An argument was empty, oversized, or had a forbidden character.</summary>
        InvalidArgument,

        /// <summary>Several accounts hold a ticket and Arcane has not recorded which is signed in.</summary>
        AmbiguousSession,

        /// <summary>A call was made before <see cref="Arcane.Init()"/> succeeded.</summary>
        NotInitialized,

        /// <summary>A filesystem or path failure inside the SDK.</summary>
        Internal,

        /// <summary>
        /// Raised by this package, not the SDK: the native plugin could not be
        /// loaded. Build it and drop it in the package's <c>Runtime/Plugins</c>
        /// folder.
        /// </summary>
        PluginMissing,

        /// <summary>
        /// Raised by this package, not the SDK: the SDK answered with something
        /// this version cannot read.
        /// </summary>
        InvalidResponse,
    }

    /// <summary>
    /// A failure, as the SDK describes it: a stable code to branch on, a message
    /// safe to show a player, and a hint plus context for your logs.
    /// </summary>
    public sealed class ArcaneError
    {
        private static readonly Dictionary<string, string> NoContext = new Dictionary<string, string>(0);

        private ArcaneError(
            ArcaneErrorCode code,
            string codeName,
            string message,
            string hint,
            bool retryable,
            Dictionary<string, string> context)
        {
            Code = code;
            CodeName = codeName ?? "unknown";
            Message = message ?? string.Empty;
            Hint = hint;
            Retryable = retryable;
            Context = context ?? NoContext;
        }

        /// <summary>Which failure this is, for a <c>switch</c>.</summary>
        public ArcaneErrorCode Code { get; private set; }

        /// <summary>The wire string of the code — always right to log, even for a code this package predates.</summary>
        public string CodeName { get; private set; }

        /// <summary>What happened. Safe to show a player.</summary>
        public string Message { get; private set; }

        /// <summary>What to do about it. For you, in the logs. May be <see langword="null"/>.</summary>
        public string Hint { get; private set; }

        /// <summary>Whether retrying can succeed once the player fixes something outside the game.</summary>
        public bool Retryable { get; private set; }

        /// <summary>The values involved, for debugging.</summary>
        public IDictionary<string, string> Context { get; private set; }

        /// <summary><c>code: message — hint (key=value)</c>, the same rendering the C ABI writes.</summary>
        public override string ToString()
        {
            var builder = new StringBuilder();
            builder.Append(CodeName).Append(": ").Append(Message);

            if (!string.IsNullOrEmpty(Hint))
            {
                builder.Append(" — ").Append(Hint);
            }

            if (Context.Count > 0)
            {
                builder.Append(" (");
                bool first = true;
                foreach (var entry in Context)
                {
                    if (!first)
                    {
                        builder.Append(", ");
                    }

                    builder.Append(entry.Key).Append('=').Append(entry.Value);
                    first = false;
                }

                builder.Append(')');
            }

            return builder.ToString();
        }

        /// <summary>Build an error this package raises itself, with no SDK call behind it.</summary>
        internal static ArcaneError Local(ArcaneErrorCode code, string codeName, string message, string hint)
        {
            return new ArcaneError(code, codeName, message, hint, false, null);
        }

        /// <summary>The native plugin is missing from the project.</summary>
        internal static ArcaneError PluginMissing()
        {
            return Local(
                ArcaneErrorCode.PluginMissing,
                "plugin_missing",
                "The Arcane SDK native plugin could not be loaded.",
                "Build it with bindings/unity/build-plugins.sh, which writes it to " +
                "Assets/Plugins/Arcane, then let Unity import it.");
        }

        /// <summary>A call that needs a client was made before init succeeded.</summary>
        internal static ArcaneError NotInitialized()
        {
            return Local(
                ArcaneErrorCode.NotInitialized,
                "not_initialized",
                "The Arcane SDK client is not initialised.",
                "Call Arcane.Init() once at launch before reading client state.");
        }

        /// <summary>An argument was rejected before it reached the SDK.</summary>
        internal static ArcaneError Argument(string message, string hint)
        {
            return Local(ArcaneErrorCode.InvalidArgument, "invalid_argument", message, hint);
        }

        /// <summary>The SDK wrote a document this package cannot read.</summary>
        internal static ArcaneError InvalidResponse(string message)
        {
            return Local(
                ArcaneErrorCode.InvalidResponse,
                "invalid_response",
                message,
                "Check that the native plugin and this package are from the same SDK release.");
        }

        /// <summary>
        /// Describe the failure that just happened, preferring the structured
        /// record the SDK keeps over the text it wrote into <c>err_buf</c>.
        /// </summary>
        /// <param name="errorBuffer">
        /// The <c>err_buf</c> an action call was given, or <see langword="null"/>
        /// for a getter that has none.
        /// </param>
        internal static ArcaneError Capture(byte[] errorBuffer)
        {
            ArcaneError structured = FromLastError();
            if (structured != null)
            {
                return structured;
            }

            string rendered = ArcaneBuffer.DecodeCString(errorBuffer);
            if (!string.IsNullOrEmpty(rendered))
            {
                return FromRendered(rendered);
            }

            return Local(
                ArcaneErrorCode.Unknown,
                "unknown",
                "The Arcane SDK reported a failure with no detail.",
                null);
        }

        /// <summary>
        /// The last failure the SDK recorded, or <see langword="null"/> when
        /// nothing has failed since the last success.
        /// </summary>
        internal static ArcaneError FromLastError()
        {
            if (!ArcaneNative.IsAvailable)
            {
                return null;
            }

            string json;
            int written = ArcaneBuffer.Read(
                ArcaneNative.arcane_sdk_last_error_json,
                ArcaneBuffer.ErrorJsonBufferSize,
                out json);

            if (written < 0)
            {
                return null;
            }

            return FromJson(json);
        }

        /// <summary>Parse the <c>{"code","message","hint","retryable","context"}</c> document.</summary>
        internal static ArcaneError FromJson(string json)
        {
            ArcaneJson root;
            if (!ArcaneJson.TryParse(json, out root) || root.Kind != ArcaneJsonKind.Object)
            {
                return null;
            }

            string codeName = root["code"].AsString("unknown");
            var context = new Dictionary<string, string>(StringComparer.Ordinal);
            foreach (var member in root["context"].Members())
            {
                context[member.Key] = member.Value.AsString(string.Empty);
            }

            return new ArcaneError(
                ParseCode(codeName),
                codeName,
                root["message"].AsString(string.Empty),
                root["hint"].AsString(),
                root["retryable"].AsBool(),
                context);
        }

        /// <summary>
        /// Best-effort parse of the <c>code: message — hint (key=value)</c>
        /// rendering, for the rare case where the structured record is gone.
        /// </summary>
        internal static ArcaneError FromRendered(string rendered)
        {
            string codeName = "unknown";
            string rest = rendered;

            int separator = rendered.IndexOf(": ", StringComparison.Ordinal);
            if (separator > 0)
            {
                codeName = rendered.Substring(0, separator);
                rest = rendered.Substring(separator + 2);
            }

            string hint = null;
            int hintAt = rest.IndexOf(" — ", StringComparison.Ordinal);
            if (hintAt >= 0)
            {
                hint = rest.Substring(hintAt + 3);
                rest = rest.Substring(0, hintAt);
            }

            return new ArcaneError(ParseCode(codeName), codeName, rest, hint, false, null);
        }

        /// <summary>Map a wire code to its enum member, or <see cref="ArcaneErrorCode.Unknown"/>.</summary>
        public static ArcaneErrorCode ParseCode(string codeName)
        {
            switch (codeName)
            {
                case "ticket_missing": return ArcaneErrorCode.TicketMissing;
                case "ticket_expired": return ArcaneErrorCode.TicketExpired;
                case "ticket_invalid": return ArcaneErrorCode.TicketInvalid;
                case "device_mismatch": return ArcaneErrorCode.DeviceMismatch;
                case "clock_rollback": return ArcaneErrorCode.ClockRollback;
                case "not_owned": return ArcaneErrorCode.NotOwned;
                case "network_required": return ArcaneErrorCode.NetworkRequired;
                case "not_authenticated": return ArcaneErrorCode.NotAuthenticated;
                case "arcane_unavailable": return ArcaneErrorCode.ArcaneUnavailable;
                case "feature_unavailable": return ArcaneErrorCode.FeatureUnavailable;
                case "unknown_achievement": return ArcaneErrorCode.UnknownAchievement;
                case "lobby_not_found": return ArcaneErrorCode.LobbyNotFound;
                case "lobby_full": return ArcaneErrorCode.LobbyFull;
                case "lobby_closed": return ArcaneErrorCode.LobbyClosed;
                case "not_friends": return ArcaneErrorCode.NotFriends;
                case "missing_game_id": return ArcaneErrorCode.MissingGameId;
                case "invalid_game_id": return ArcaneErrorCode.InvalidGameId;
                case "invalid_argument": return ArcaneErrorCode.InvalidArgument;
                case "ambiguous_session": return ArcaneErrorCode.AmbiguousSession;
                case "not_initialized": return ArcaneErrorCode.NotInitialized;
                case "internal": return ArcaneErrorCode.Internal;
                case "plugin_missing": return ArcaneErrorCode.PluginMissing;
                case "invalid_response": return ArcaneErrorCode.InvalidResponse;
                default: return ArcaneErrorCode.Unknown;
            }
        }
    }

    /// <summary>
    /// Thrown by the methods that do not hand you an
    /// <see cref="ArcaneError"/> — every one of them has a <c>Try…</c> twin
    /// that returns <see langword="false"/> instead.
    /// </summary>
    public sealed class ArcaneException : Exception
    {
        internal ArcaneException(ArcaneError error)
            : base(error == null ? "The Arcane SDK reported a failure." : error.ToString())
        {
            Error = error;
        }

        /// <summary>The failure, with its code, hint and context.</summary>
        public ArcaneError Error { get; private set; }
    }
}
