// SPDX-License-Identifier: MIT OR Apache-2.0
#if UNITY_5_3_OR_NEWER
using ArcanePowered.Native;
using UnityEngine;

namespace ArcanePowered
{
    /// <summary>
    /// What <see cref="ArcaneRuntime"/> does on your behalf, as a project asset.
    /// </summary>
    /// <remarks>
    /// Create it from <b>Assets ▸ Create ▸ Arcane Powered ▸ Settings</b>, or from
    /// <b>Project Settings ▸ Arcane Powered</b>. It must live in a
    /// <c>Resources</c> folder under <c>Assets</c> — a package's own folder is
    /// read-only once the package is installed, so the asset belongs to your
    /// project. Without one, the defaults below apply and everything still
    /// works.
    /// </remarks>
    [CreateAssetMenu(fileName = ResourceName, menuName = "Arcane Powered/Settings", order = 0)]
    public sealed class ArcaneSettings : ScriptableObject
    {
        /// <summary>The name to load with <see cref="Resources.Load(string)"/>.</summary>
        public const string ResourceName = "ArcaneSettings";

        private static ArcaneSettings _instance;

        [Header("Lifecycle")]
        [Tooltip("Call Arcane.Init() before the first scene loads. Turn this off to check ownership yourself, " +
                 "for example behind a splash screen you control.")]
        [SerializeField]
        private bool autoInitialize = true;

        [Tooltip("Call Arcane.Shutdown() when the game quits, which reports the final playtime.")]
        [SerializeField]
        private bool shutdownOnQuit = true;

        [Header("Session")]
        [Tooltip("Call Arcane.Frame() every frame, which is what FPS sampling counts.")]
        [SerializeField]
        private bool countFrames = true;

        [Tooltip("Report the resolution and quality preset to attach to FPS samples, and again whenever they change.")]
        [SerializeField]
        private bool reportGraphicsSettings = true;

        [Header("Lobbies")]
        [Tooltip("Drain the lobby event queue and raise the Arcane.Lobbies events on the main thread. " +
                 "Turn this off to call Arcane.Lobbies.PollEvents() yourself.")]
        [SerializeField]
        private bool pumpLobbyEvents = true;

        [Tooltip("Seconds between two drains of the lobby event queue. The queue is filled by the SDK's own " +
                 "thread, so this only decides how quickly your game notices.")]
        [SerializeField]
        [Range(0.1f, 10f)]
        private float lobbyPollInterval = 1f;

        [Header("Editor")]
        [Tooltip("The game id from the Arcane portal, used in the Editor only. Arcane Powered sets ARCANE_GAME_ID " +
                 "itself when it launches a shipped build; nothing sets it for the Editor.")]
        [SerializeField]
        private string editorGameId = string.Empty;

        [Tooltip("Which account's ticket to read, in the Editor only. Leave it empty to follow whichever account " +
                 "is signed in to the Arcane desktop app.")]
        [SerializeField]
        private string editorUserId = string.Empty;

        /// <summary>
        /// The project's settings, or the defaults when the project has none.
        /// </summary>
        public static ArcaneSettings Instance
        {
            get
            {
                if (_instance != null)
                {
                    return _instance;
                }

                _instance = Resources.Load<ArcaneSettings>(ResourceName);
                if (_instance == null)
                {
                    _instance = CreateInstance<ArcaneSettings>();
                    _instance.hideFlags = HideFlags.HideAndDontSave;
                }

                return _instance;
            }
        }

        /// <summary>Whether to call <see cref="Arcane.Init()"/> before the first scene loads.</summary>
        public bool AutoInitialize
        {
            get { return autoInitialize; }
        }

        /// <summary>Whether to call <see cref="Arcane.Shutdown"/> when the game quits.</summary>
        public bool ShutdownOnQuit
        {
            get { return shutdownOnQuit; }
        }

        /// <summary>Whether to call <see cref="Arcane.Frame"/> every frame.</summary>
        public bool CountFrames
        {
            get { return countFrames; }
        }

        /// <summary>Whether to report the resolution and quality preset for FPS samples.</summary>
        public bool ReportGraphicsSettings
        {
            get { return reportGraphicsSettings; }
        }

        /// <summary>Whether to drain the lobby event queue and raise the <see cref="Arcane.Lobbies"/> events.</summary>
        public bool PumpLobbyEvents
        {
            get { return pumpLobbyEvents; }
        }

        /// <summary>Seconds between two drains of the lobby event queue.</summary>
        public float LobbyPollInterval
        {
            get { return lobbyPollInterval; }
        }

        /// <summary>The game id to run under in the Editor.</summary>
        public string EditorGameId
        {
            get { return editorGameId; }
        }

        /// <summary>The account to run as in the Editor, or empty to follow the desktop app.</summary>
        public string EditorUserId
        {
            get { return editorUserId; }
        }

        /// <summary>
        /// Put <see cref="EditorGameId"/> and <see cref="EditorUserId"/> in the
        /// process environment, where the SDK reads them.
        /// </summary>
        /// <remarks>
        /// Editor only, and only for values you filled in: a shipped build gets
        /// its ids from the launcher, and an empty field is left alone so a real
        /// launch is never overwritten. Variables already set in the environment
        /// the Editor was started with win, so a launch profile still decides.
        /// </remarks>
        internal void ApplyEditorEnvironment()
        {
#if UNITY_EDITOR
            Apply("ARCANE_GAME_ID", editorGameId);
            Apply("ARCANE_USER_ID", editorUserId);
#endif
        }

#if UNITY_EDITOR
        private static void Apply(string name, string value)
        {
            if (string.IsNullOrEmpty(value))
            {
                return;
            }

            if (!string.IsNullOrEmpty(System.Environment.GetEnvironmentVariable(name)))
            {
                return;
            }

            if (!ArcaneEnvironment.SetProcessVariable(name, value.Trim()))
            {
                ArcaneLog.Warning(
                    "Could not set " + name + " for the native SDK. Set it in the environment that starts " +
                    "the Unity Editor instead.");
            }
        }
#endif
    }
}
#endif
