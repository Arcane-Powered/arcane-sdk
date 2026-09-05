// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using ArcanePowered.Json;
using ArcanePowered.Native;

namespace ArcanePowered
{
    /// <summary>One of this player's friends on Arcane, with their presence.</summary>
    public sealed class ArcaneFriend
    {
        private ArcaneFriend()
        {
        }

        /// <summary>Their Arcane account id — what <see cref="ArcaneLobbies.Invite"/> takes.</summary>
        public string UserId { get; private set; }

        /// <summary>Their display name on Arcane.</summary>
        public string Pseudo { get; private set; }

        /// <summary>Whether they are online.</summary>
        public bool Online { get; private set; }

        /// <summary>Whether they are playing <em>this</em> title right now.</summary>
        public bool InGame { get; private set; }

        internal static ArcaneFriend FromJson(ArcaneJson node)
        {
            return new ArcaneFriend
            {
                UserId = node["user_id"].AsString(string.Empty),
                Pseudo = node["pseudo"].AsString(string.Empty),
                Online = node["online"].AsBool(),
                InGame = node["in_game"].AsBool(),
            };
        }
    }

    /// <summary>A friends list, and whether it came from the desktop app's cache.</summary>
    public sealed class ArcaneFriendList
    {
        internal ArcaneFriendList(ArcaneFriend[] friends, bool stale)
        {
            Friends = friends;
            Stale = stale;
        }

        /// <summary>The friends, in the order Arcane returned them.</summary>
        public ArcaneFriend[] Friends { get; private set; }

        /// <summary>
        /// <see langword="true"/> when the desktop app answered from its cache
        /// because it is offline. The list is still usable.
        /// </summary>
        public bool Stale { get; private set; }

        internal static ArcaneFriendList Empty()
        {
            return new ArcaneFriendList(Array.Empty<ArcaneFriend>(), false);
        }
    }

    /// <summary>
    /// This player's friends. Reached through <see cref="Arcane.Friends"/>.
    /// </summary>
    /// <remarks>
    /// <see cref="List"/> makes one synchronous loopback call — call it when a
    /// menu opens or on a timer of your own, never from <c>Update</c>. The
    /// desktop app caches the answer for 15 seconds, so a tighter timer buys
    /// nothing.
    /// </remarks>
    public sealed class ArcaneFriends
    {
        internal ArcaneFriends()
        {
        }

        /// <summary>The friends list, answering <see langword="false"/> instead of throwing.</summary>
        public bool TryList(out ArcaneFriendList friends, out ArcaneError error)
        {
            friends = ArcaneFriendList.Empty();

            ArcaneJson root;
            if (!ArcaneCall.ReadJson(
                    ArcaneNative.arcane_sdk_friends_json,
                    ArcaneBuffer.ListBufferSize,
                    out root,
                    out error))
            {
                return false;
            }

            var parsed = new List<ArcaneFriend>();
            foreach (var node in root["friends"].Items)
            {
                parsed.Add(ArcaneFriend.FromJson(node));
            }

            friends = new ArcaneFriendList(parsed.ToArray(), root["stale"].AsBool());
            return true;
        }

        /// <summary>The friends list, throwing <see cref="ArcaneException"/> on failure.</summary>
        public ArcaneFriendList List()
        {
            ArcaneFriendList friends;
            ArcaneError error;
            if (!TryList(out friends, out error))
            {
                throw new ArcaneException(error);
            }

            return friends;
        }

        /// <summary>The friends list, fetched on a background thread.</summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers an empty list and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<ArcaneFriendList> ListAsync()
        {
            return Task.Run(() =>
            {
                ArcaneFriendList friends;
                ArcaneError ignored;
                return TryList(out friends, out ignored) ? friends : ArcaneFriendList.Empty();
            });
        }
    }
}
