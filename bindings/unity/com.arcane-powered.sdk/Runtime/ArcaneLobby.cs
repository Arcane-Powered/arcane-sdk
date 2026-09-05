// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Collections.Generic;
using ArcanePowered.Json;

namespace ArcanePowered
{
    /// <summary>Who can join a lobby. Arcane never lists lobbies publicly.</summary>
    public enum ArcaneLobbyVisibility
    {
        /// <summary>The host's friends see it in the launcher and can join. No join code is issued.</summary>
        Friends = 0,

        /// <summary>Only players who have the six-character join code can join.</summary>
        Code = 1,

        /// <summary>Both: friends see it, and the code works for anyone who has it.</summary>
        FriendsAndCode = 2,
    }

    /// <summary>One player in a lobby, with the connection blob their game published.</summary>
    public sealed class ArcaneLobbyMember
    {
        private ArcaneLobbyMember()
        {
        }

        /// <summary>Their Arcane account id.</summary>
        public string UserId { get; private set; }

        /// <summary>Their display name on Arcane.</summary>
        public string Pseudo { get; private set; }

        /// <summary>Whatever their copy of the game passed when it created or joined the lobby.</summary>
        public byte[] Payload { get; private set; }

        internal static ArcaneLobbyMember FromJson(ArcaneJson node)
        {
            return new ArcaneLobbyMember
            {
                UserId = node["user_id"].AsString(string.Empty),
                Pseudo = node["pseudo"].AsString(string.Empty),
                Payload = ArcaneLobby.DecodePayload(node["payload"].AsString()),
            };
        }
    }

    /// <summary>
    /// A lobby as Arcane knows it, right after a create, a join or a read.
    /// </summary>
    /// <remarks>
    /// A snapshot, not a live view: players who arrive later come through the
    /// lobby events as <see cref="ArcaneLobbyEventType.MemberJoined"/>. Arcane
    /// hosts the lobby and the join code; the connection itself is your
    /// netcode's business, and the payloads are bytes Arcane never reads.
    /// </remarks>
    public sealed class ArcaneLobby
    {
        /// <summary>The largest connection blob Arcane accepts, before base64.</summary>
        public const int MaxPayloadBytes = 4096;

        private ArcaneLobby()
        {
        }

        /// <summary>Arcane's id for the lobby — what invite, leave and close take.</summary>
        public string LobbyId { get; private set; }

        /// <summary>
        /// The six-character code to show the player, or <see langword="null"/>
        /// for a friends-only lobby and for a member who is not the host.
        /// </summary>
        public string JoinCode { get; private set; }

        /// <summary>The Arcane account hosting the lobby.</summary>
        public string HostUserId { get; private set; }

        /// <summary>The host's connection blob — what a joining player connects to.</summary>
        public byte[] HostPayload { get; private set; }

        /// <summary>Everyone in the lobby right now, host included.</summary>
        public ArcaneLobbyMember[] Members { get; private set; }

        /// <summary>The capacity the host asked for.</summary>
        public int MaxPlayers { get; private set; }

        internal static ArcaneLobby FromJson(ArcaneJson root)
        {
            var members = new List<ArcaneLobbyMember>();
            foreach (var node in root["members"].Items)
            {
                members.Add(ArcaneLobbyMember.FromJson(node));
            }

            return new ArcaneLobby
            {
                LobbyId = root["lobby_id"].AsString(string.Empty),
                JoinCode = root["join_code"].AsString(),
                HostUserId = root["host_user_id"].AsString(string.Empty),
                HostPayload = DecodePayload(root["host_payload"].AsString()),
                Members = members.ToArray(),
                MaxPlayers = (int)root["max_players"].AsLong(),
            };
        }

        /// <summary>
        /// Decode a payload the SDK wrote. A blob Arcane could not encode comes
        /// back empty rather than throwing into the caller's frame.
        /// </summary>
        internal static byte[] DecodePayload(string base64)
        {
            if (string.IsNullOrEmpty(base64))
            {
                return Array.Empty<byte>();
            }

            try
            {
                return Convert.FromBase64String(base64);
            }
            catch (FormatException)
            {
                return Array.Empty<byte>();
            }
        }
    }

    /// <summary>What kind of thing happened in a lobby.</summary>
    public enum ArcaneLobbyEventType
    {
        /// <summary>An event type this package does not know — ignore it.</summary>
        Unknown = 0,

        /// <summary>A friend invited this player to their lobby.</summary>
        Invite,

        /// <summary>Somebody joined a lobby this player is in.</summary>
        MemberJoined,

        /// <summary>Somebody left.</summary>
        MemberLeft,

        /// <summary>The lobby is over. There is no host migration.</summary>
        LobbyClosed,

        /// <summary>
        /// Arcane dropped events before this client fetched them. Re-read the
        /// lobbies you are in with <see cref="ArcaneLobbies.Get"/>.
        /// </summary>
        Resync,
    }

    /// <summary>
    /// Something that happened in a lobby this player is in, or an invitation.
    /// </summary>
    /// <remarks>
    /// Which fields carry a value depends on <see cref="Type"/>:
    /// <see cref="ArcaneLobbyEventType.Invite"/> has
    /// <see cref="JoinCode"/>, <see cref="FromUserId"/> and
    /// <see cref="Pseudo"/>; <see cref="ArcaneLobbyEventType.MemberJoined"/>
    /// has <see cref="UserId"/>, <see cref="Pseudo"/> and
    /// <see cref="Payload"/>; <see cref="ArcaneLobbyEventType.MemberLeft"/> has
    /// <see cref="UserId"/>; <see cref="ArcaneLobbyEventType.Resync"/> has no
    /// <see cref="LobbyId"/> at all, because it is about all of them.
    /// </remarks>
    public sealed class ArcaneLobbyEvent
    {
        private ArcaneLobbyEvent()
        {
        }

        /// <summary>Which event this is.</summary>
        public ArcaneLobbyEventType Type { get; private set; }

        /// <summary>The lobby it is about, or <see langword="null"/> for a resync.</summary>
        public string LobbyId { get; private set; }

        /// <summary>The join code that came with an invite, when the lobby issues one.</summary>
        public string JoinCode { get; private set; }

        /// <summary>Who sent the invite.</summary>
        public string FromUserId { get; private set; }

        /// <summary>Who joined or left.</summary>
        public string UserId { get; private set; }

        /// <summary>The display name of the player this event is about.</summary>
        public string Pseudo { get; private set; }

        /// <summary>The connection blob of the player who joined — connect to it.</summary>
        public byte[] Payload { get; private set; }

        internal static ArcaneLobbyEvent FromJson(ArcaneJson node)
        {
            return new ArcaneLobbyEvent
            {
                Type = ParseType(node["type"].AsString()),
                LobbyId = node["lobby_id"].AsString(),
                JoinCode = node["join_code"].AsString(),
                FromUserId = node["from_user_id"].AsString(),
                UserId = node["user_id"].AsString(),
                Pseudo = node["pseudo"].AsString(),
                Payload = ArcaneLobby.DecodePayload(node["payload"].AsString()),
            };
        }

        private static ArcaneLobbyEventType ParseType(string value)
        {
            switch (value)
            {
                case "invite": return ArcaneLobbyEventType.Invite;
                case "member_joined": return ArcaneLobbyEventType.MemberJoined;
                case "member_left": return ArcaneLobbyEventType.MemberLeft;
                case "lobby_closed": return ArcaneLobbyEventType.LobbyClosed;
                case "resync": return ArcaneLobbyEventType.Resync;
                default: return ArcaneLobbyEventType.Unknown;
            }
        }
    }
}
