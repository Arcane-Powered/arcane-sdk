// SPDX-License-Identifier: MIT OR Apache-2.0
using ArcanePowered;
using UnityEngine;

/// <summary>
/// What a boot scene does with the SDK: read the ownership check that already
/// ran, tell the player what to do when it failed, and get on with the game.
/// </summary>
/// <remarks>
/// Nothing here calls <see cref="Arcane.Init()"/>: with the default settings the
/// runtime has already done it before this scene loaded, which is why the result
/// is waiting in <see cref="ArcaneRuntime.InitializationError"/>. Turn
/// <c>Auto Initialize</c> off in <b>Project Settings ▸ Arcane Powered</b> if you
/// would rather run the check yourself behind a splash screen.
/// </remarks>
public sealed class ArcaneQuickStart : MonoBehaviour
{
    [SerializeField]
    [Tooltip("An achievement key from the Arcane portal, unlocked with the button below.")]
    private string achievementKey = "first_blood";

    private string _status = "…";
    private ArcaneAchievement[] _achievements;

    private void Start()
    {
        ArcaneError error = ArcaneRuntime.InitializationError;

        if (Arcane.IsInitialized)
        {
            _status = "Signed in as " + Arcane.UserId + " — " + Arcane.Ownership;
            LoadAchievements();
            return;
        }

        if (error == null)
        {
            // Auto Initialize is off, so nothing has checked ownership yet.
            _status = "Not initialised yet — call Arcane.Init() when your boot flow is ready.";
            return;
        }

        // The ownership check is the interesting failure: what the player should
        // do about it depends entirely on which one it is.
        switch (error.Code)
        {
            case ArcaneErrorCode.NotOwned:
                _status = "This account does not own the game. Send the player to the store.";
                break;
            case ArcaneErrorCode.ArcaneUnavailable:
                _status = "Start the Arcane Powered desktop app, then retry.";
                break;
            case ArcaneErrorCode.NotAuthenticated:
                _status = "Sign in to the Arcane Powered desktop app, then retry.";
                break;
            case ArcaneErrorCode.MissingGameId:
                _status = "No game id. In the Editor, set one in Project Settings ▸ Arcane Powered.";
                break;
            default:
                _status = error.Message;
                break;
        }

        // Retryable failures are the ones a player can fix without restarting:
        // start the desktop app, sign in, reconnect. Offer the button.
        if (error.Retryable)
        {
            _status += "\n(retryable)";
        }

        Debug.LogWarning("[Arcane] " + error);
    }

    private void OnGUI()
    {
        GUILayout.BeginArea(new Rect(16f, 16f, 460f, 320f));
        GUILayout.Label(_status);

        if (Arcane.IsInitialized)
        {
            ArcaneSessionInfo session = Arcane.Session;
            GUILayout.Label(
                "Session: " + session.Tracking + " · " + session.PlayedSeconds + "s · " +
                (session.LastFpsAverage.HasValue ? session.LastFpsAverage.Value.ToString("F1") + " fps" : "no sample yet"));

            if (GUILayout.Button("Unlock " + achievementKey))
            {
                UnlockAchievement();
            }

            if (_achievements != null)
            {
                foreach (ArcaneAchievement achievement in _achievements)
                {
                    GUILayout.Label((achievement.IsUnlocked ? "✓ " : "· ") + achievement.Title);
                }
            }
        }
        else if (ArcaneRuntime.InitializationError != null && ArcaneRuntime.InitializationError.Retryable)
        {
            if (GUILayout.Button("Retry"))
            {
                Retry();
            }
        }

        GUILayout.EndArea();
    }

    /// <summary>
    /// Unlocking is idempotent, so the safe thing is to call it every time the
    /// condition holds and never track whether you already did.
    /// </summary>
    private async void UnlockAchievement()
    {
        // One loopback call: off the main thread, so a slow desktop app cannot
        // cost a frame.
        bool unlocked = await Arcane.Achievements.UnlockAsync(achievementKey);

        // …and back, because everything below touches the scene.
        Arcane.RunOnMainThread(() =>
        {
            _status = unlocked ? "Unlocked " + achievementKey : "Could not unlock: " + Arcane.LastError;
            LoadAchievements();
        });
    }

    private async void LoadAchievements()
    {
        ArcaneAchievement[] achievements = await Arcane.Achievements.ListAsync();
        Arcane.RunOnMainThread(() => _achievements = achievements);
    }

    private void Retry()
    {
        ArcaneError error;
        _status = Arcane.TryInit(out error)
            ? "Signed in as " + Arcane.UserId
            : error.Message;
    }
}
