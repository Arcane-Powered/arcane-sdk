// SPDX-License-Identifier: MIT OR Apache-2.0
using System.Collections.Generic;
using System.Text;
using ArcanePowered;
using UnityEngine;

/// <summary>
/// Hosting and joining with Arcane as the matchmaker: it holds the lobby, the
/// membership and the join code, and hands every player the connection blobs of
/// the others. Connecting to them is your netcode's job — this sample just shows
/// what arrives.
/// </summary>
/// <remarks>
/// The payload here is a made-up endpoint string. In a real game it is whatever
/// your transport needs to reach a peer: a host endpoint, a relay ticket, a
/// Steam id. Arcane never reads it, and it must stay under
/// <see cref="ArcaneLobby.MaxPayloadBytes"/> bytes.
/// </remarks>
public sealed class ArcaneLobbySample : MonoBehaviour
{
    private const int MaxPlayers = 4;

    private readonly List<string> _log = new List<string>();

    private ArcaneLobby _lobby;
    private string _joinCodeInput = string.Empty;

    private void OnEnable()
    {
        // Raised on the main thread by the runtime, which drains the SDK's queue
        // once a second. If you would rather pump it yourself, turn Pump Lobby
        // Events off in Project Settings and call Arcane.Lobbies.PollEvents().
        Arcane.Lobbies.InviteReceived += OnInvite;
        Arcane.Lobbies.MemberJoined += OnMemberJoined;
        Arcane.Lobbies.MemberLeft += OnMemberLeft;
        Arcane.Lobbies.LobbyClosed += OnLobbyClosed;
        Arcane.Lobbies.ResyncRequested += OnResync;
    }

    private void OnDisable()
    {
        Arcane.Lobbies.InviteReceived -= OnInvite;
        Arcane.Lobbies.MemberJoined -= OnMemberJoined;
        Arcane.Lobbies.MemberLeft -= OnMemberLeft;
        Arcane.Lobbies.LobbyClosed -= OnLobbyClosed;
        Arcane.Lobbies.ResyncRequested -= OnResync;
    }

    private void Start()
    {
        // The player clicked "Join" on a friend in the launcher, and the game was
        // started with their code. Honour it before showing a menu.
        string launchCode = Arcane.Lobbies.LaunchJoinCode;
        if (!string.IsNullOrEmpty(launchCode))
        {
            Log("Launched with join code " + launchCode);
            JoinByCode(launchCode);
        }
    }

    private void OnGUI()
    {
        GUILayout.BeginArea(new Rect(16f, 16f, 520f, 460f));

        if (!Arcane.IsInitialized)
        {
            GUILayout.Label("The SDK is not initialised — see the Quick start sample.");
            GUILayout.EndArea();
            return;
        }

        if (_lobby == null)
        {
            DrawLobbyMenu();
        }
        else
        {
            DrawLobby();
        }

        GUILayout.Space(12f);
        foreach (string line in _log)
        {
            GUILayout.Label(line);
        }

        GUILayout.EndArea();
    }

    private void DrawLobbyMenu()
    {
        if (GUILayout.Button("Host a lobby"))
        {
            Host();
        }

        GUILayout.BeginHorizontal();
        _joinCodeInput = GUILayout.TextField(_joinCodeInput, 6, GUILayout.Width(96f));
        if (GUILayout.Button("Join by code"))
        {
            JoinByCode(_joinCodeInput);
        }

        GUILayout.EndHorizontal();
    }

    private void DrawLobby()
    {
        GUILayout.Label("Lobby " + _lobby.LobbyId + "  ·  " + _lobby.Members.Length + "/" + _lobby.MaxPlayers);

        // Null for a friends-only lobby, and for anyone who is not the host.
        if (!string.IsNullOrEmpty(_lobby.JoinCode))
        {
            GUILayout.Label("Join code: " + _lobby.JoinCode);
        }

        foreach (ArcaneLobbyMember member in _lobby.Members)
        {
            GUILayout.Label("· " + member.Pseudo + "  →  " + Encoding.UTF8.GetString(member.Payload));
        }

        if (GUILayout.Button("Invite the first friend who is online"))
        {
            InviteAFriend();
        }

        if (GUILayout.Button("Leave"))
        {
            Leave();
        }
    }

    private async void Host()
    {
        // Visibility decides who can reach it: friends see FriendsAndCode
        // lobbies in the launcher, and the six-character code works for anyone
        // the player sends it to.
        ArcaneLobby lobby = await Arcane.Lobbies.CreateAsync(
            MaxPlayers,
            ArcaneLobbyVisibility.FriendsAndCode,
            MyConnectionBlob());

        Arcane.RunOnMainThread(() =>
        {
            if (lobby == null)
            {
                Log("Could not host: " + Arcane.LastError);
                return;
            }

            _lobby = lobby;
            Log("Hosting — code " + lobby.JoinCode);
        });
    }

    private async void JoinByCode(string code)
    {
        ArcaneLobby lobby = await Arcane.Lobbies.JoinByCodeAsync(code, MyConnectionBlob());

        Arcane.RunOnMainThread(() =>
        {
            if (lobby == null)
            {
                // lobby_not_found for a mistyped or ended lobby, lobby_full when
                // somebody took the last seat, not_friends for a friends-only one.
                Log("Could not join: " + Arcane.LastError);
                return;
            }

            _lobby = lobby;
            Log("Joined " + lobby.LobbyId + " — connecting to " + Encoding.UTF8.GetString(lobby.HostPayload));
            ConnectTo(lobby.HostPayload);
        });
    }

    private async void InviteAFriend()
    {
        ArcaneFriendList friends = await Arcane.Friends.ListAsync();
        string lobbyId = _lobby.LobbyId;

        foreach (ArcaneFriend friend in friends.Friends)
        {
            if (!friend.Online)
            {
                continue;
            }

            bool invited = await Arcane.Lobbies.InviteAsync(lobbyId, friend.UserId);
            Arcane.RunOnMainThread(() => Log(invited ? "Invited " + friend.Pseudo : "Invite failed: " + Arcane.LastError));
            return;
        }

        Arcane.RunOnMainThread(() => Log("No friend is online."));
    }

    private async void Leave()
    {
        string lobbyId = _lobby.LobbyId;
        _lobby = null;
        await Arcane.Lobbies.LeaveAsync(lobbyId);
        Arcane.RunOnMainThread(() => Log("Left " + lobbyId));
    }

    private void OnInvite(ArcaneLobbyEvent lobbyEvent)
    {
        Log(lobbyEvent.Pseudo + " invited you — " + (lobbyEvent.JoinCode ?? lobbyEvent.LobbyId));
    }

    private void OnMemberJoined(ArcaneLobbyEvent lobbyEvent)
    {
        Log(lobbyEvent.Pseudo + " joined");
        ConnectTo(lobbyEvent.Payload);
        Refresh();
    }

    private void OnMemberLeft(ArcaneLobbyEvent lobbyEvent)
    {
        Log(lobbyEvent.UserId + " left");
        Refresh();
    }

    private void OnLobbyClosed(ArcaneLobbyEvent lobbyEvent)
    {
        // There is no host migration: the lobby is over for everyone.
        Log("The lobby closed");
        _lobby = null;
    }

    private void OnResync(ArcaneLobbyEvent lobbyEvent)
    {
        // Arcane dropped events before this client fetched them, so the member
        // list built from events may have a hole in it. Ask instead of guessing.
        Log("Resync — re-reading the lobby");
        Refresh();
    }

    private async void Refresh()
    {
        if (_lobby == null)
        {
            return;
        }

        ArcaneLobby lobby = await Arcane.Lobbies.GetAsync(_lobby.LobbyId);
        Arcane.RunOnMainThread(() =>
        {
            if (lobby != null)
            {
                _lobby = lobby;
            }
        });
    }

    /// <summary>Whatever your transport needs to reach this player.</summary>
    private static byte[] MyConnectionBlob()
    {
        return Encoding.UTF8.GetBytes("udp://" + SystemInfo.deviceUniqueIdentifier + ":7777");
    }

    /// <summary>Where your netcode takes over.</summary>
    private void ConnectTo(byte[] payload)
    {
        if (payload == null || payload.Length == 0)
        {
            return;
        }

        Debug.Log("[Arcane] connect to " + Encoding.UTF8.GetString(payload));
    }

    private void Log(string line)
    {
        _log.Insert(0, line);
        if (_log.Count > 10)
        {
            _log.RemoveAt(_log.Count - 1);
        }
    }
}
