// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Collections.Generic;
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
