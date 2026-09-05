// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using ArcanePowered.Json;
using ArcanePowered.Native;

namespace ArcanePowered
{
    /// <summary>What the local cache knows about one achievement.</summary>
    public enum ArcaneUnlockState
    {
        /// <summary>The list has never been loaded, or it did not carry this key.</summary>
        Unknown = 0,

        /// <summary>Locked as of the last loaded list.</summary>
        Locked,

        /// <summary>Unlocked.</summary>
        Unlocked,
    }

    /// <summary>One achievement of this title, as the Arcane portal defines it.</summary>
    public sealed class ArcaneAchievement
    {
        private ArcaneAchievement()
        {
        }

        /// <summary>The key you pass to <see cref="ArcaneAchievements.Unlock"/>.</summary>
        public string Key { get; private set; }

        /// <summary>Display name.</summary>
        public string Title { get; private set; }

        /// <summary>Display description. Empty for a hidden achievement the player has not unlocked.</summary>
        public string Description { get; private set; }

        /// <summary>Icon URL, or <see langword="null"/> when the portal has none.</summary>
        public string IconUrl { get; private set; }

        /// <summary>Whether the portal hides this one until it is unlocked.</summary>
        public bool Hidden { get; private set; }

        /// <summary>When it was unlocked, or <see langword="null"/> while it is locked.</summary>
        public DateTimeOffset? UnlockedAt { get; private set; }

        /// <summary>Whether this player has it.</summary>
        public bool IsUnlocked
        {
            get { return UnlockedAt.HasValue; }
        }

        internal static ArcaneAchievement FromJson(ArcaneJson node)
        {
            long? unlockedAt = node["unlocked_at"].AsNullableLong();

            return new ArcaneAchievement
            {
                Key = node["key"].AsString(string.Empty),
                Title = node["title"].AsString(string.Empty),
                Description = node["description"].AsString(string.Empty),
                IconUrl = node["icon_url"].AsString(),
                Hidden = node["hidden"].AsBool(),
                UnlockedAt = unlockedAt.HasValue
                    ? DateTimeOffset.FromUnixTimeSeconds(unlockedAt.Value)
                    : (DateTimeOffset?)null,
            };
        }
    }

    /// <summary>
    /// Achievements for the signed-in player. Reached through
    /// <see cref="Arcane.Achievements"/>.
    /// </summary>
    /// <remarks>
    /// <see cref="Unlock"/> and <see cref="List"/> each make one synchronous
    /// loopback call to the Arcane desktop app — call them on a loading screen,
    /// an achievements screen or a background task, never from
    /// <c>Update</c>. <see cref="IsUnlocked"/> reads memory and is free.
    /// </remarks>
    public sealed class ArcaneAchievements
    {
        internal ArcaneAchievements()
        {
        }

        /// <summary>
        /// Unlock an achievement, answering <see langword="false"/> instead of
        /// throwing.
        /// </summary>
        /// <remarks>
        /// Idempotent: call it every time the condition holds. An
        /// already-unlocked achievement, and one queued because the desktop app
        /// is offline, both succeed.
        /// </remarks>
        public bool TryUnlock(string key, out ArcaneError error)
        {
            if (string.IsNullOrEmpty(key))
            {
                error = ArcaneError.Argument(
                    "An achievement key is required.",
                    "Pass the key from the Arcane portal — lowercase, up to 64 bytes.");
                return false;
            }

            byte[] encoded = ArcaneBuffer.ToUtf8(key);
            return ArcaneCall.Run(
                (buffer, length) => ArcaneNative.arcane_sdk_achievement_unlock(encoded, buffer, length),
                out error);
        }

        /// <summary>Unlock an achievement, throwing <see cref="ArcaneException"/> on failure.</summary>
        public void Unlock(string key)
        {
            ArcaneError error;
            if (!TryUnlock(key, out error))
            {
                throw new ArcaneException(error);
            }
        }

        /// <summary>
        /// Unlock an achievement on a background thread.
        /// </summary>
        /// <remarks>
        /// The returned task completes off Unity's main thread. Await it from a
        /// method that does not touch the scene, or hand the result back through
        /// <see cref="Arcane.RunOnMainThread"/>. A failure answers
        /// <see langword="false"/> and leaves the reason in
        /// <see cref="Arcane.LastError"/> — use <see cref="TryUnlock"/> to be handed
        /// the error itself.
        /// </remarks>
        public Task<bool> UnlockAsync(string key)
        {
            return Task.Run(() =>
            {
                ArcaneError ignored;
                return TryUnlock(key, out ignored);
            });
        }

        /// <summary>
        /// Every achievement of this title, answering <see langword="false"/>
        /// instead of throwing.
        /// </summary>
        /// <remarks>
        /// One synchronous loopback call. It also fills the cache
        /// <see cref="IsUnlocked"/> reads, so call it once on a loading screen
        /// if you want that check to answer.
        /// </remarks>
        public bool TryList(out ArcaneAchievement[] achievements, out ArcaneError error)
        {
            achievements = Array.Empty<ArcaneAchievement>();

            ArcaneJson root;
            if (!ArcaneCall.ReadJson(
                    ArcaneNative.arcane_sdk_achievements_json,
                    ArcaneBuffer.ListBufferSize,
                    out root,
                    out error))
            {
                return false;
            }

            var parsed = new List<ArcaneAchievement>();
            foreach (var node in root["achievements"].Items)
            {
                parsed.Add(ArcaneAchievement.FromJson(node));
            }

            achievements = parsed.ToArray();
            return true;
        }

        /// <summary>Every achievement of this title, throwing on failure.</summary>
        public ArcaneAchievement[] List()
        {
            ArcaneAchievement[] achievements;
            ArcaneError error;
            if (!TryList(out achievements, out error))
            {
                throw new ArcaneException(error);
            }

            return achievements;
        }

        /// <summary>
        /// Every achievement of this title, fetched on a background thread.
        /// </summary>
        /// <remarks>
        /// Completes off Unity's main thread, so hand the result back with
        /// <see cref="Arcane.RunOnMainThread"/> before you touch the scene. A failure
        /// answers an empty list and leaves the reason in <see cref="Arcane.LastError"/> —
        /// use the <c>Try…</c> form to be handed the error itself.
        /// </remarks>
        public Task<ArcaneAchievement[]> ListAsync()
        {
            return Task.Run(() =>
            {
                ArcaneAchievement[] achievements;
                ArcaneError ignored;
                return TryList(out achievements, out ignored) ? achievements : Array.Empty<ArcaneAchievement>();
            });
        }

        /// <summary>
        /// What the cache says about one key — memory only, no call.
        /// </summary>
        /// <remarks>
        /// The cache is filled by <see cref="List"/> and updated by
        /// <see cref="Unlock"/>, so before the first list this answers
        /// <see cref="ArcaneUnlockState.Unknown"/>.
        /// </remarks>
        public ArcaneUnlockState GetUnlockState(string key)
        {
            if (string.IsNullOrEmpty(key) || !ArcaneNative.IsAvailable)
            {
                return ArcaneUnlockState.Unknown;
            }

            switch (ArcaneNative.arcane_sdk_achievement_is_unlocked(ArcaneBuffer.ToUtf8(key)))
            {
                case 1: return ArcaneUnlockState.Unlocked;
                case 0: return ArcaneUnlockState.Locked;
                default: return ArcaneUnlockState.Unknown;
            }
        }

        /// <summary>
        /// Whether the cache has this achievement unlocked. An unknown key
        /// reads as <see langword="false"/> — use
        /// <see cref="GetUnlockState"/> when the difference matters.
        /// </summary>
        public bool IsUnlocked(string key)
        {
            return GetUnlockState(key) == ArcaneUnlockState.Unlocked;
        }
    }
}
