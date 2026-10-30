// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;
using System.Threading.Tasks;
using ArcanePowered.Json;
using ArcanePowered.Native;

namespace ArcanePowered
{
    /// <summary>
    /// Lobbies and invitations. Reached through <see cref="Arcane.Lobbies"/>.
    /// </summary>
    /// <remarks>
    /// Arcane hosts the lobby, the membership and the join code; connecting the
    /// players is your netcode's job. Every method here except
    /// <see cref="PollEvents"/> and <see cref="LaunchJoinCode"/> makes one
    /// synchronous loopback call — never call them from <c>Update</c>; the
    /// <c>…Async</c> twins run them on a background thread.
    ///
    /// The first lobby call arms the SDK's background thread, which from then on
    /// polls Arcane for events every five seconds.
    /// </remarks>
    public sealed class ArcaneLobbies
    {
        internal ArcaneLobbies()
        {
        }

        /// <summary>A friend invited this player to their lobby.</summary>
        /// <remarks>
        /// Raised on Unity's main thread by <see cref="ArcaneRuntime"/> while
        /// <see cref="ArcaneSettings.PumpLobbyEvents"/> is on. With the pump
        /// off, drain the queue yourself with <see cref="PollEvents"/>.
        /// </remarks>
        public event Action<ArcaneLobbyEvent> InviteReceived;

        /// <summary>Somebody joined a lobby this player is in — connect to their payload.</summary>
        /// <remarks>Raised on Unity's main thread, like <see cref="InviteReceived"/>.</remarks>
        public event Action<ArcaneLobbyEvent> MemberJoined;

        /// <summary>Somebody left.</summary>
        /// <remarks>Raised on Unity's main thread, like <see cref="InviteReceived"/>.</remarks>
        public event Action<ArcaneLobbyEvent> MemberLeft;

        /// <summary>The lobby is over — the host closed it or their session expired.</summary>
        /// <remarks>Raised on Unity's main thread, like <see cref="InviteReceived"/>.</remarks>
        public event Action<ArcaneLobbyEvent> LobbyClosed;

        /// <summary>The host changed the lobby's metadata — read <see cref="ArcaneLobbyEvent.Metadata"/>.</summary>
        /// <remarks>Raised on Unity's main thread, like <see cref="InviteReceived"/>.</remarks>
        public event Action<ArcaneLobbyEvent> LobbyUpdated;

        /// <summary>
        /// Arcane dropped events before this client fetched them: re-read the
        /// lobbies you are in with <see cref="Get"/> instead of trusting what
        /// the earlier events built up.
        /// </summary>
        /// <remarks>Raised on Unity's main thread, like <see cref="InviteReceived"/>.</remarks>
        public event Action<ArcaneLobbyEvent> ResyncRequested;

        /// <summary>
        /// The join code this game was launched with, when the player started it
        /// from a friend's "Join" in the launcher. <see langword="null"/> when
        /// there is none.
        /// </summary>
        /// <remarks>
        /// Read from the desktop app on the first call and cached for the
        /// process, so checking it once at boot is enough.
        /// </remarks>
        public string LaunchJoinCode
        {
            get { return ArcaneCall.ReadValue(ArcaneNative.arcane_sdk_launch_join_code, 16); }
        }

        // --- Create and join ----------------------------------------------

        /// <summary>
        /// Open a lobby with this player as its host, answering
        /// <see langword="false"/> instead of throwing.
        /// </summary>
        /// <param name="maxPlayers">Capacity, 1 to 255, host included.</param>
        /// <param name="visibility">Who can join.</param>
        /// <param name="payload">
        /// Your connection blob — an endpoint, a relay token, whatever your
        /// netcode needs — at most <see cref="ArcaneLobby.MaxPayloadBytes"/>
        /// bytes. <see langword="null"/> for none. The wrapper base64-encodes it
        /// for you.
        /// </param>
        public bool TryCreate(
            int maxPlayers,
            ArcaneLobbyVisibility visibility,
            byte[] payload,
            out ArcaneLobby lobby,
            out ArcaneError error)
        {
            lobby = null;

            if (maxPlayers < 1 || maxPlayers > 255)
            {
                error = ArcaneError.Argument(
                    "A lobby holds between 1 and 255 players.",
                    "Pass the capacity your game supports, host included.");
                return false;
            }

            byte[] encoded;
            if (!TryEncodePayload(payload, out encoded, out error))
            {
                return false;
            }

            byte capacity = (byte)maxPlayers;
            int rawVisibility = (int)visibility;

            return ReadLobby(
                (buffer, length) =>
                    ArcaneNative.arcane_sdk_lobby_create(capacity, rawVisibility, encoded, buffer, length),
                out lobby,
                out error);
        }

        /// <summary>Open a lobby, throwing <see cref="ArcaneException"/> on failure.</summary>
        public ArcaneLobby Create(int maxPlayers, ArcaneLobbyVisibility visibility, byte[] payload)
        {
            ArcaneLobby lobby;
            ArcaneError error;
            if (!TryCreate(maxPlayers, visibility, payload, out lobby, out error))
            {
                throw new ArcaneException(error);
            }

            return lobby;
        }

        /// <summary>Open a lobby on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers <see langword="null"/> and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<ArcaneLobby> CreateAsync(int maxPlayers, ArcaneLobbyVisibility visibility, byte[] payload)
        {
            return Task.Run(() =>
            {
                ArcaneLobby lobby;
                ArcaneError ignored;
                TryCreate(maxPlayers, visibility, payload, out lobby, out ignored);
                return lobby;
            });
        }

        /// <summary>
        /// Open a lobby from <paramref name="options"/> — the form that takes a
        /// public visibility and metadata — answering <see langword="false"/>
        /// instead of throwing.
        /// </summary>
        public bool TryCreate(ArcaneLobbyOptions options, out ArcaneLobby lobby, out ArcaneError error)
        {
            lobby = null;

            if (options == null)
            {
                error = ArcaneError.Argument("Lobby options are required.", null);
                return false;
            }

            if (options.MaxPlayers < 2 || options.MaxPlayers > 64)
            {
                error = ArcaneError.Argument(
                    "A lobby holds between 2 and 64 players.",
                    "Pass the capacity your game supports, host included.");
                return false;
            }

            if (!TryCheckMetadata(options.Metadata, out error))
            {
                return false;
            }

            byte[] encoded;
            if (!TryEncodePayload(options.Payload, out encoded, out error))
            {
                return false;
            }

            var json = new StringBuilder("{\"max_players\":");
            json.Append(options.MaxPlayers.ToString(CultureInfo.InvariantCulture));
            json.Append(",\"visibility\":");
            AppendString(json, VisibilityName(options.Visibility));
            if (encoded != null)
            {
                json.Append(",\"payload\":");
                AppendString(json, Convert.ToBase64String(options.Payload));
            }

            json.Append(",\"metadata\":");
            AppendObject(json, options.Metadata);
            json.Append('}');

            byte[] body = ArcaneBuffer.ToUtf8(json.ToString());
            return ReadLobby(
                (buffer, length) => ArcaneNative.arcane_sdk_lobby_create_json(body, buffer, length),
                out lobby,
                out error);
        }

        /// <summary>Open a lobby from options, throwing <see cref="ArcaneException"/> on failure.</summary>
        public ArcaneLobby Create(ArcaneLobbyOptions options)
        {
            ArcaneLobby lobby;
            ArcaneError error;
            if (!TryCreate(options, out lobby, out error))
            {
                throw new ArcaneException(error);
            }

            return lobby;
        }

        /// <summary>Open a lobby from options on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers <see langword="null"/> and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<ArcaneLobby> CreateAsync(ArcaneLobbyOptions options)
        {
            return Task.Run(() =>
            {
                ArcaneLobby lobby;
                ArcaneError ignored;
                TryCreate(options, out lobby, out ignored);
                return lobby;
            });
        }

        /// <summary>
        /// List the open public lobbies of this game, answering
        /// <see langword="false"/> instead of throwing.
        /// </summary>
        /// <remarks>
        /// Lobbies hosted by a friend come first, then the newest. Pass
        /// <see langword="null"/> for no filter, free slots only, 20 results.
        /// </remarks>
        public bool TrySearch(ArcaneLobbyQuery query, out ArcaneLobbySummary[] lobbies, out ArcaneError error)
        {
            lobbies = Array.Empty<ArcaneLobbySummary>();
            query = query ?? new ArcaneLobbyQuery();

            if (query.Limit < 1 || query.Limit > ArcaneLobbyQuery.MaxResults)
            {
                error = ArcaneError.Argument(
                    "A search answers between 1 and " + ArcaneLobbyQuery.MaxResults + " lobbies.",
                    null);
                return false;
            }

            if (!TryCheckMetadata(query.Filters, out error))
            {
                return false;
            }

            var json = new StringBuilder("{\"filters\":");
            AppendObject(json, query.Filters);
            json.Append(",\"has_free_slot\":");
            json.Append(query.HasFreeSlot ? "true" : "false");
            json.Append(",\"limit\":");
            json.Append(query.Limit.ToString(CultureInfo.InvariantCulture));
            json.Append('}');

            byte[] body = ArcaneBuffer.ToUtf8(json.ToString());
            ArcaneJson root;
            if (!ArcaneCall.ReadJson(
                    (buffer, length) => ArcaneNative.arcane_sdk_lobby_search(body, buffer, length),
                    ArcaneBuffer.ListBufferSize,
                    out root,
                    out error))
            {
                return false;
            }

            var found = new List<ArcaneLobbySummary>();
            foreach (var node in root["lobbies"].Items)
            {
                found.Add(ArcaneLobbySummary.FromJson(node));
            }

            lobbies = found.ToArray();
            return true;
        }

        /// <summary>List the open public lobbies, throwing on failure.</summary>
        public ArcaneLobbySummary[] Search(ArcaneLobbyQuery query)
        {
            ArcaneLobbySummary[] lobbies;
            ArcaneError error;
            if (!TrySearch(query, out lobbies, out error))
            {
                throw new ArcaneException(error);
            }

            return lobbies;
        }

        /// <summary>List the open public lobbies on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers an empty array and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<ArcaneLobbySummary[]> SearchAsync(ArcaneLobbyQuery query)
        {
            return Task.Run(() =>
            {
                ArcaneLobbySummary[] lobbies;
                ArcaneError ignored;
                TrySearch(query, out lobbies, out ignored);
                return lobbies;
            });
        }

        /// <summary>
        /// Replace the metadata of a lobby this player hosts, answering
        /// <see langword="false"/> instead of throwing.
        /// </summary>
        /// <remarks>
        /// The whole set, not a patch: an empty dictionary clears it. The other
        /// members get <see cref="LobbyUpdated"/>.
        /// </remarks>
        public bool TrySetMetadata(
            string lobbyId,
            IDictionary<string, string> metadata,
            out ArcaneLobby lobby,
            out ArcaneError error)
        {
            lobby = null;

            if (string.IsNullOrEmpty(lobbyId))
            {
                error = ArcaneError.Argument("A lobby id is required.", null);
                return false;
            }

            if (!TryCheckMetadata(metadata, out error))
            {
                return false;
            }

            var json = new StringBuilder("{\"metadata\":");
            AppendObject(json, metadata);
            json.Append('}');

            byte[] id = ArcaneBuffer.ToUtf8(lobbyId);
            byte[] body = ArcaneBuffer.ToUtf8(json.ToString());
            return ReadLobby(
                (buffer, length) => ArcaneNative.arcane_sdk_lobby_set_metadata(id, body, buffer, length),
                out lobby,
                out error);
        }

        /// <summary>Replace a lobby's metadata, throwing on failure.</summary>
        public ArcaneLobby SetMetadata(string lobbyId, IDictionary<string, string> metadata)
        {
            ArcaneLobby lobby;
            ArcaneError error;
            if (!TrySetMetadata(lobbyId, metadata, out lobby, out error))
            {
                throw new ArcaneException(error);
            }

            return lobby;
        }

        /// <summary>Replace a lobby's metadata on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers <see langword="null"/> and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<ArcaneLobby> SetMetadataAsync(string lobbyId, IDictionary<string, string> metadata)
        {
            return Task.Run(() =>
            {
                ArcaneLobby lobby;
                ArcaneError ignored;
                TrySetMetadata(lobbyId, metadata, out lobby, out ignored);
                return lobby;
            });
        }

        /// <summary>
        /// Join a lobby by id — what an invite carries — answering
        /// <see langword="false"/> instead of throwing.
        /// </summary>
        public bool TryJoin(string lobbyId, byte[] payload, out ArcaneLobby lobby, out ArcaneError error)
        {
            lobby = null;

            if (string.IsNullOrEmpty(lobbyId))
            {
                error = ArcaneError.Argument("A lobby id is required.", "Use the id an invite event carried.");
                return false;
            }

            byte[] encoded;
            if (!TryEncodePayload(payload, out encoded, out error))
            {
                return false;
            }

            byte[] id = ArcaneBuffer.ToUtf8(lobbyId);
            return ReadLobby(
                (buffer, length) => ArcaneNative.arcane_sdk_lobby_join(id, encoded, buffer, length),
                out lobby,
                out error);
        }

        /// <summary>Join a lobby by id, throwing on failure.</summary>
        public ArcaneLobby Join(string lobbyId, byte[] payload)
        {
            ArcaneLobby lobby;
            ArcaneError error;
            if (!TryJoin(lobbyId, payload, out lobby, out error))
            {
                throw new ArcaneException(error);
            }

            return lobby;
        }

        /// <summary>Join a lobby by id on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers <see langword="null"/> and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<ArcaneLobby> JoinAsync(string lobbyId, byte[] payload)
        {
            return Task.Run(() =>
            {
                ArcaneLobby lobby;
                ArcaneError ignored;
                TryJoin(lobbyId, payload, out lobby, out ignored);
                return lobby;
            });
        }

        /// <summary>
        /// Join the lobby a six-character code points at, answering
        /// <see langword="false"/> instead of throwing.
        /// </summary>
        /// <remarks>The code is uppercased before it is checked.</remarks>
        public bool TryJoinByCode(string joinCode, byte[] payload, out ArcaneLobby lobby, out ArcaneError error)
        {
            lobby = null;

            if (string.IsNullOrEmpty(joinCode))
            {
                error = ArcaneError.Argument(
                    "A join code is required.",
                    "Join codes are six characters of A–H J–N P–Z 2–9.");
                return false;
            }

            byte[] encoded;
            if (!TryEncodePayload(payload, out encoded, out error))
            {
                return false;
            }

            byte[] code = ArcaneBuffer.ToUtf8(joinCode);
            return ReadLobby(
                (buffer, length) => ArcaneNative.arcane_sdk_lobby_join_code(code, encoded, buffer, length),
                out lobby,
                out error);
        }

        /// <summary>Join by code, throwing on failure.</summary>
        public ArcaneLobby JoinByCode(string joinCode, byte[] payload)
        {
            ArcaneLobby lobby;
            ArcaneError error;
            if (!TryJoinByCode(joinCode, payload, out lobby, out error))
            {
                throw new ArcaneException(error);
            }

            return lobby;
        }

        /// <summary>Join by code on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers <see langword="null"/> and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<ArcaneLobby> JoinByCodeAsync(string joinCode, byte[] payload)
        {
            return Task.Run(() =>
            {
                ArcaneLobby lobby;
                ArcaneError ignored;
                TryJoinByCode(joinCode, payload, out lobby, out ignored);
                return lobby;
            });
        }

        /// <summary>
        /// Read a lobby as Arcane knows it right now, joining nothing and
        /// leaving nothing.
        /// </summary>
        /// <remarks>
        /// This is what a <see cref="ArcaneLobbyEventType.Resync"/> asks you to
        /// do, and what to reach for whenever you would rather ask than replay
        /// events.
        /// </remarks>
        public bool TryGet(string lobbyId, out ArcaneLobby lobby, out ArcaneError error)
        {
            lobby = null;

            if (string.IsNullOrEmpty(lobbyId))
            {
                error = ArcaneError.Argument("A lobby id is required.", null);
                return false;
            }

            byte[] id = ArcaneBuffer.ToUtf8(lobbyId);
            return ReadLobby(
                (buffer, length) => ArcaneNative.arcane_sdk_lobby_get(id, buffer, length),
                out lobby,
                out error);
        }

        /// <summary>Read a lobby, throwing on failure.</summary>
        public ArcaneLobby Get(string lobbyId)
        {
            ArcaneLobby lobby;
            ArcaneError error;
            if (!TryGet(lobbyId, out lobby, out error))
            {
                throw new ArcaneException(error);
            }

            return lobby;
        }

        /// <summary>Read a lobby on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers <see langword="null"/> and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<ArcaneLobby> GetAsync(string lobbyId)
        {
            return Task.Run(() =>
            {
                ArcaneLobby lobby;
                ArcaneError ignored;
                TryGet(lobbyId, out lobby, out ignored);
                return lobby;
            });
        }

        // --- Membership ----------------------------------------------------

        /// <summary>Invite one friend, answering <see langword="false"/> instead of throwing.</summary>
        public bool TryInvite(string lobbyId, string toUserId, out ArcaneError error)
        {
            if (string.IsNullOrEmpty(lobbyId) || string.IsNullOrEmpty(toUserId))
            {
                error = ArcaneError.Argument(
                    "A lobby id and the friend's user id are both required.",
                    "User ids come from Arcane.Friends.List().");
                return false;
            }

            byte[] id = ArcaneBuffer.ToUtf8(lobbyId);
            byte[] target = ArcaneBuffer.ToUtf8(toUserId);
            return ArcaneCall.Run(
                (buffer, length) => ArcaneNative.arcane_sdk_lobby_invite(id, target, buffer, length),
                out error);
        }

        /// <summary>Invite one friend, throwing on failure.</summary>
        public void Invite(string lobbyId, string toUserId)
        {
            ArcaneError error;
            if (!TryInvite(lobbyId, toUserId, out error))
            {
                throw new ArcaneException(error);
            }
        }

        /// <summary>Invite one friend on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers <see langword="false"/> and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<bool> InviteAsync(string lobbyId, string toUserId)
        {
            return Task.Run(() =>
            {
                ArcaneError ignored;
                return TryInvite(lobbyId, toUserId, out ignored);
            });
        }

        /// <summary>
        /// Leave a lobby, answering <see langword="false"/> instead of throwing.
        /// For the host this ends it — there is no host migration.
        /// </summary>
        public bool TryLeave(string lobbyId, out ArcaneError error)
        {
            if (string.IsNullOrEmpty(lobbyId))
            {
                error = ArcaneError.Argument("A lobby id is required.", null);
                return false;
            }

            byte[] id = ArcaneBuffer.ToUtf8(lobbyId);
            return ArcaneCall.Run(
                (buffer, length) => ArcaneNative.arcane_sdk_lobby_leave(id, buffer, length),
                out error);
        }

        /// <summary>Leave a lobby, throwing on failure.</summary>
        public void Leave(string lobbyId)
        {
            ArcaneError error;
            if (!TryLeave(lobbyId, out error))
            {
                throw new ArcaneException(error);
            }
        }

        /// <summary>Leave a lobby on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers <see langword="false"/> and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<bool> LeaveAsync(string lobbyId)
        {
            return Task.Run(() =>
            {
                ArcaneError ignored;
                return TryLeave(lobbyId, out ignored);
            });
        }

        /// <summary>
        /// Close a lobby this player hosts, answering <see langword="false"/>
        /// instead of throwing. Its members get a
        /// <see cref="ArcaneLobbyEventType.LobbyClosed"/> event.
        /// </summary>
        public bool TryClose(string lobbyId, out ArcaneError error)
        {
            if (string.IsNullOrEmpty(lobbyId))
            {
                error = ArcaneError.Argument("A lobby id is required.", null);
                return false;
            }

            byte[] id = ArcaneBuffer.ToUtf8(lobbyId);
            return ArcaneCall.Run(
                (buffer, length) => ArcaneNative.arcane_sdk_lobby_close(id, buffer, length),
                out error);
        }

        /// <summary>Close a lobby you host, throwing on failure.</summary>
        public void Close(string lobbyId)
        {
            ArcaneError error;
            if (!TryClose(lobbyId, out error))
            {
                throw new ArcaneException(error);
            }
        }

        /// <summary>Close a lobby you host, on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers <see langword="false"/> and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<bool> CloseAsync(string lobbyId)
        {
            return Task.Run(() =>
            {
                ArcaneError ignored;
                return TryClose(lobbyId, out ignored);
            });
        }

        // --- Events --------------------------------------------------------

        /// <summary>
        /// Take the lobby events collected since the last call, oldest first.
        /// </summary>
        /// <remarks>
        /// Memory only — the SDK's background thread does the polling — so this
        /// is cheap enough to call once a second. It <em>drains</em> the queue:
        /// let <see cref="ArcaneRuntime"/> pump it and subscribe to
        /// <see cref="InviteReceived"/> and friends, or turn
        /// <see cref="ArcaneSettings.PumpLobbyEvents"/> off and call this
        /// yourself — doing both means each event reaches only one of them.
        /// </remarks>
        public ArcaneLobbyEvent[] PollEvents()
        {
            ArcaneJson root;
            ArcaneError error;
            if (!ArcaneCall.ReadJson(
                    ArcaneNative.arcane_sdk_lobby_events_json,
                    ArcaneBuffer.ListBufferSize,
                    out root,
                    out error))
            {
                return Array.Empty<ArcaneLobbyEvent>();
            }

            var events = new List<ArcaneLobbyEvent>();
            foreach (var node in root["events"].Items)
            {
                events.Add(ArcaneLobbyEvent.FromJson(node));
            }

            return events.ToArray();
        }

        /// <summary>
        /// Drain the queue and raise the C# events. Called by
        /// <see cref="ArcaneRuntime"/> on the main thread.
        /// </summary>
        internal void PumpEvents()
        {
            foreach (var lobbyEvent in PollEvents())
            {
                Dispatch(lobbyEvent);
            }
        }

        private void Dispatch(ArcaneLobbyEvent lobbyEvent)
        {
            switch (lobbyEvent.Type)
            {
                case ArcaneLobbyEventType.Invite:
                    Raise(InviteReceived, lobbyEvent);
                    break;
                case ArcaneLobbyEventType.MemberJoined:
                    Raise(MemberJoined, lobbyEvent);
                    break;
                case ArcaneLobbyEventType.MemberLeft:
                    Raise(MemberLeft, lobbyEvent);
                    break;
                case ArcaneLobbyEventType.LobbyClosed:
                    Raise(LobbyClosed, lobbyEvent);
                    break;
                case ArcaneLobbyEventType.LobbyUpdated:
                    Raise(LobbyUpdated, lobbyEvent);
                    break;
                case ArcaneLobbyEventType.Resync:
                    Raise(ResyncRequested, lobbyEvent);
                    break;
            }
        }

        /// <summary>
        /// Raise one handler chain. A subscriber that throws must not swallow
        /// the events queued behind it, so the failure is reported and the pump
        /// carries on.
        /// </summary>
        private static void Raise(Action<ArcaneLobbyEvent> handler, ArcaneLobbyEvent lobbyEvent)
        {
            if (handler == null)
            {
                return;
            }

            try
            {
                handler(lobbyEvent);
            }
            catch (Exception exception)
            {
                ArcaneLog.Exception(exception);
            }
        }

        // --- Plumbing ------------------------------------------------------

        private static bool ReadLobby(ArcaneBuffer.Getter getter, out ArcaneLobby lobby, out ArcaneError error)
        {
            lobby = null;

            ArcaneJson root;
            if (!ArcaneCall.ReadJson(getter, ArcaneBuffer.ListBufferSize, out root, out error))
            {
                return false;
            }

            lobby = ArcaneLobby.FromJson(root);
            return true;
        }

        private static string VisibilityName(ArcaneLobbyVisibility visibility)
        {
            switch (visibility)
            {
                case ArcaneLobbyVisibility.Code: return "code";
                case ArcaneLobbyVisibility.FriendsAndCode: return "friends_and_code";
                case ArcaneLobbyVisibility.Public: return "public";
                default: return "friends";
            }
        }

        private static bool TryCheckMetadata(IDictionary<string, string> metadata, out ArcaneError error)
        {
            error = null;
            if (metadata == null)
            {
                return true;
            }

            if (metadata.Count > ArcaneLobbyOptions.MaxMetadataPairs)
            {
                error = ArcaneError.Argument(
                    "A lobby carries at most " + ArcaneLobbyOptions.MaxMetadataPairs + " metadata pairs.",
                    "Keep the few values players search on: a mode, a map, a region.");
                return false;
            }

            foreach (var pair in metadata)
            {
                if (!IsMetadataKey(pair.Key) || pair.Value == null || pair.Value.Length > 128)
                {
                    error = ArcaneError.Argument(
                        "The lobby metadata pair \"" + pair.Key + "\" is malformed.",
                        "Keys are 1–32 characters of a–z, 0–9 and _; values at most 128 characters.");
                    return false;
                }
            }

            return true;
        }

        private static bool IsMetadataKey(string key)
        {
            if (string.IsNullOrEmpty(key) || key.Length > 32)
            {
                return false;
            }

            foreach (char c in key)
            {
                bool allowed = (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '_';
                if (!allowed)
                {
                    return false;
                }
            }

            return true;
        }

        private static void AppendObject(StringBuilder json, IDictionary<string, string> pairs)
        {
            json.Append('{');
            if (pairs != null)
            {
                bool first = true;
                foreach (var pair in pairs)
                {
                    if (!first)
                    {
                        json.Append(',');
                    }

                    first = false;
                    AppendString(json, pair.Key);
                    json.Append(':');
                    AppendString(json, pair.Value);
                }
            }

            json.Append('}');
        }

        private static void AppendString(StringBuilder json, string value)
        {
            json.Append('"');
            foreach (char c in value)
            {
                switch (c)
                {
                    case '"': json.Append("\\\""); break;
                    case '\\': json.Append("\\\\"); break;
                    case '\n': json.Append("\\n"); break;
                    case '\r': json.Append("\\r"); break;
                    case '\t': json.Append("\\t"); break;
                    default:
                        if (c < 0x20)
                        {
                            json.Append("\\u").Append(((int)c).ToString("x4", CultureInfo.InvariantCulture));
                        }
                        else
                        {
                            json.Append(c);
                        }

                        break;
                }
            }

            json.Append('"');
        }

        private static bool TryEncodePayload(byte[] payload, out byte[] encoded, out ArcaneError error)
        {
            error = null;

            if (payload == null || payload.Length == 0)
            {
                // A null pointer is how the C ABI spells "no payload"; an empty
                // array would encode to an empty string, which is not the same.
                encoded = null;
                return true;
            }

            if (payload.Length > ArcaneLobby.MaxPayloadBytes)
            {
                encoded = null;
                error = ArcaneError.Argument(
                    "A lobby payload is at most " + ArcaneLobby.MaxPayloadBytes + " bytes, got " + payload.Length + ".",
                    "Publish an endpoint or a token, not your game state.");
                return false;
            }

            encoded = ArcaneBuffer.ToUtf8(Convert.ToBase64String(payload));
            return true;
        }
    }
}
