// SPDX-License-Identifier: MIT OR Apache-2.0
using System.Collections.Generic;
using System.Text;
using ArcanePowered.Json;
using NUnit.Framework;

namespace ArcanePowered.Tests
{
    /// <summary>
    /// The documents in these tests are the ones the C ABI reference documents,
    /// verbatim — if the SDK ever changes shape, this is where it shows.
    /// </summary>
    public sealed class ArcaneModelTests
    {
        [Test]
        public void ReadsAnActiveSession()
        {
            ArcaneSessionInfo session = ArcaneSessionInfo.FromJson(ArcaneJson.Parse(
                "{\"session_id\":\"s-1\",\"tracking\":\"active\",\"played_seconds\":1830,\"fps_sampling\":true," +
                "\"samples_taken\":6,\"last_fps_avg\":59.8,\"lobby_events\":\"active\"}"));

            Assert.AreEqual("s-1", session.SessionId);
            Assert.AreEqual(ArcaneTracking.Active, session.Tracking);
            Assert.AreEqual(1830, session.PlayedSeconds);
            Assert.IsTrue(session.FpsSampling);
            Assert.AreEqual(6, session.SamplesTaken);
            Assert.AreEqual(59.8f, session.LastFpsAverage);
            Assert.AreEqual(ArcaneLobbyPolling.Active, session.LobbyEvents);
        }

        [Test]
        public void ReadsASessionThatHasNotStartedYet()
        {
            ArcaneSessionInfo session = ArcaneSessionInfo.FromJson(ArcaneJson.Parse(
                "{\"session_id\":null,\"tracking\":\"pending\",\"played_seconds\":0,\"fps_sampling\":false," +
                "\"samples_taken\":0,\"last_fps_avg\":null,\"lobby_events\":\"off\"}"));

            Assert.IsNull(session.SessionId);
            Assert.AreEqual(ArcaneTracking.Pending, session.Tracking);
            Assert.IsNull(session.LastFpsAverage);
            Assert.AreEqual(ArcaneLobbyPolling.Off, session.LobbyEvents);
        }

        [Test]
        public void AnUnlockedAchievementCarriesItsTimestamp()
        {
            ArcaneAchievement achievement = ArcaneAchievement.FromJson(ArcaneJson.Parse(
                "{\"key\":\"first_blood\",\"title\":\"First blood\",\"description\":\"d\"," +
                "\"icon_url\":\"https://cdn/i.png\",\"hidden\":false,\"unlocked_at\":1777638896}"));

            Assert.AreEqual("first_blood", achievement.Key);
            Assert.IsTrue(achievement.IsUnlocked);
            Assert.AreEqual(1777638896, achievement.UnlockedAt.Value.ToUnixTimeSeconds());
        }

        [Test]
        public void ALockedAchievementHasNoTimestampAndMayHaveNoIcon()
        {
            ArcaneAchievement achievement = ArcaneAchievement.FromJson(ArcaneJson.Parse(
                "{\"key\":\"k\",\"title\":\"t\",\"description\":\"\",\"icon_url\":null," +
                "\"hidden\":true,\"unlocked_at\":null}"));

            Assert.IsFalse(achievement.IsUnlocked);
            Assert.IsNull(achievement.UnlockedAt);
            Assert.IsNull(achievement.IconUrl);
            Assert.IsTrue(achievement.Hidden);
        }

        [Test]
        public void ReadsAFriendWithANonAsciiPseudo()
        {
            ArcaneFriend friend = ArcaneFriend.FromJson(ArcaneJson.Parse(
                "{\"user_id\":\"u-1\",\"pseudo\":\"Zo\\u00e9\",\"online\":true,\"in_game\":false}"));

            Assert.AreEqual("Zoé", friend.Pseudo);
            Assert.IsTrue(friend.Online);
            Assert.IsFalse(friend.InGame);
        }

        [Test]
        public void ReadsALobbyAndDecodesItsPayloads()
        {
            ArcaneLobby lobby = ArcaneLobby.FromJson(ArcaneJson.Parse(
                "{\"lobby_id\":\"l-1\",\"join_code\":\"K7P3QX\",\"host_user_id\":\"u-1\"," +
                "\"host_payload\":\"dWRwOi8v\",\"members\":[{\"user_id\":\"u-1\",\"pseudo\":\"A\"," +
                "\"payload\":\"dWRwOi8v\"}],\"max_players\":4}"));

            Assert.AreEqual("K7P3QX", lobby.JoinCode);
            Assert.AreEqual("udp://", Encoding.UTF8.GetString(lobby.HostPayload));
            Assert.AreEqual(1, lobby.Members.Length);
            Assert.AreEqual("udp://", Encoding.UTF8.GetString(lobby.Members[0].Payload));
            Assert.AreEqual(4, lobby.MaxPlayers);
        }

        [Test]
        public void AFriendsOnlyLobbyHasNoJoinCode()
        {
            ArcaneLobby lobby = ArcaneLobby.FromJson(ArcaneJson.Parse(
                "{\"lobby_id\":\"l-2\",\"join_code\":null,\"host_user_id\":\"u-1\",\"host_payload\":\"\"," +
                "\"members\":[],\"max_players\":2}"));

            Assert.IsNull(lobby.JoinCode);
            Assert.IsEmpty(lobby.HostPayload);
        }

        [Test]
        public void ReadsEveryEventKind()
        {
            ArcaneJson document = ArcaneJson.Parse(
                "{\"events\":[" +
                "{\"type\":\"invite\",\"lobby_id\":\"l-1\",\"join_code\":\"K7P3QX\",\"from_user_id\":\"u-2\",\"pseudo\":\"B\"}," +
                "{\"type\":\"member_joined\",\"lobby_id\":\"l-1\",\"user_id\":\"u-3\",\"pseudo\":\"C\",\"payload\":\"dWRwOi8v\"}," +
                "{\"type\":\"member_left\",\"lobby_id\":\"l-1\",\"user_id\":\"u-3\"}," +
                "{\"type\":\"lobby_closed\",\"lobby_id\":\"l-1\"}," +
                "{\"type\":\"resync\"}]}");

            var events = new List<ArcaneLobbyEvent>();
            foreach (ArcaneJson node in document["events"].Items)
            {
                events.Add(ArcaneLobbyEvent.FromJson(node));
            }

            Assert.AreEqual(ArcaneLobbyEventType.Invite, events[0].Type);
            Assert.AreEqual("u-2", events[0].FromUserId);
            Assert.AreEqual("K7P3QX", events[0].JoinCode);
            Assert.AreEqual(ArcaneLobbyEventType.MemberJoined, events[1].Type);
            Assert.AreEqual("udp://", Encoding.UTF8.GetString(events[1].Payload));
            Assert.AreEqual(ArcaneLobbyEventType.MemberLeft, events[2].Type);
            Assert.AreEqual(ArcaneLobbyEventType.LobbyClosed, events[3].Type);
            Assert.AreEqual(ArcaneLobbyEventType.Resync, events[4].Type);
            Assert.IsNull(events[4].LobbyId, "a resync is about every lobby, so it names none");
        }

        [Test]
        public void AnEventKindThisPackagePredatesIsIgnorable()
        {
            ArcaneLobbyEvent unknown = ArcaneLobbyEvent.FromJson(
                ArcaneJson.Parse("{\"type\":\"lobby_renamed\",\"lobby_id\":\"l-1\"}"));

            Assert.AreEqual(ArcaneLobbyEventType.Unknown, unknown.Type);
        }

        [Test]
        public void ReadsAStructuredError()
        {
            ArcaneError error = ArcaneError.FromJson(
                "{\"code\":\"not_owned\",\"message\":\"m\",\"hint\":\"h\",\"retryable\":false," +
                "\"context\":{\"game_id\":\"g\"}}");

            Assert.AreEqual(ArcaneErrorCode.NotOwned, error.Code);
            Assert.AreEqual("not_owned", error.CodeName);
            Assert.AreEqual("g", error.Context["game_id"]);
            Assert.IsFalse(error.Retryable);
            Assert.AreEqual("not_owned: m — h (game_id=g)", error.ToString());
        }

        [Test]
        public void AnErrorCodeThisPackagePredatesKeepsItsWireString()
        {
            ArcaneError error = ArcaneError.FromJson(
                "{\"code\":\"a_new_code\",\"message\":\"m\",\"hint\":null,\"retryable\":true,\"context\":{}}");

            Assert.AreEqual(ArcaneErrorCode.Unknown, error.Code);
            Assert.AreEqual("a_new_code", error.CodeName);
            Assert.IsTrue(error.Retryable);
        }

        [Test]
        public void ParsesTheRenderedErrorTheErrorBufferCarries()
        {
            ArcaneError error = ArcaneError.FromRendered(
                "arcane_unavailable: Could not reach Arcane — Start the desktop app (port=39284)");

            Assert.AreEqual(ArcaneErrorCode.ArcaneUnavailable, error.Code);
            Assert.AreEqual("Could not reach Arcane", error.Message);
            Assert.AreEqual("Start the desktop app (port=39284)", error.Hint);
        }
    }
}
