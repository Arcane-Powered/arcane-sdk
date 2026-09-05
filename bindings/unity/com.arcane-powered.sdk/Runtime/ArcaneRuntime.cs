// SPDX-License-Identifier: MIT OR Apache-2.0
#if UNITY_5_3_OR_NEWER
using UnityEngine;

namespace ArcanePowered
{
    /// <summary>
    /// The component that drives the SDK for you: it initialises before the
    /// first scene, counts frames, drains the lobby queue, and shuts the session
    /// down when the game quits.
    /// </summary>
    /// <remarks>
    /// You never add this to a scene. It creates itself before the first scene
    /// loads and survives every scene change; what it does is decided by
    /// <see cref="ArcaneSettings"/>. Turn a job off there and do it yourself —
    /// the static API is the same either way.
    /// </remarks>
    [DisallowMultipleComponent]
    [AddComponentMenu("")]
    public sealed class ArcaneRuntime : MonoBehaviour
    {
        private static ArcaneRuntime _instance;

        private float _sinceLastPoll;
        private string _reportedResolution;
        private string _reportedPreset;

        /// <summary>Whether the runtime object exists and is pumping frames.</summary>
        public static bool IsRunning
        {
            get { return _instance != null; }
        }

        /// <summary>
        /// Whether the automatic <see cref="Arcane.Init()"/> has run. It is
        /// <see langword="false"/> when
        /// <see cref="ArcaneSettings.AutoInitialize"/> is off — the game owns the
        /// lifecycle then.
        /// </summary>
        public static bool AutoInitializeRan { get; private set; }

        /// <summary>
        /// Why the automatic <see cref="Arcane.Init()"/> failed, or
        /// <see langword="null"/> when it succeeded or never ran.
        /// </summary>
        /// <remarks>
        /// Read this from the first scene — a <c>Start</c> in your boot
        /// controller — to decide what to show a player who may not play:
        /// <see cref="ArcaneErrorCode.NotOwned"/> sends them to the store,
        /// <see cref="ArcaneErrorCode.ArcaneUnavailable"/> asks them to start
        /// the Arcane desktop app.
        /// </remarks>
        public static ArcaneError InitializationError { get; private set; }

        /// <summary>
        /// Create the runtime object if it does not exist yet.
        /// </summary>
        /// <remarks>
        /// Only needed when <see cref="ArcaneSettings.AutoInitialize"/> is off
        /// and you want frame counting and the lobby pump after initialising the
        /// SDK yourself.
        /// </remarks>
        public static ArcaneRuntime Ensure()
        {
            if (_instance != null)
            {
                return _instance;
            }

            var host = new GameObject("[Arcane Powered]");
            host.hideFlags = HideFlags.DontSave;
            DontDestroyOnLoad(host);
            return host.AddComponent<ArcaneRuntime>();
        }

        [RuntimeInitializeOnLoadMethod(RuntimeInitializeLoadType.BeforeSceneLoad)]
        private static void Bootstrap()
        {
            // Statics survive a domain reload in the Editor; the run they
            // described does not.
            AutoInitializeRan = false;
            InitializationError = null;

            ArcaneSettings settings = ArcaneSettings.Instance;

            if (settings.AutoInitialize)
            {
                settings.ApplyEditorEnvironment();

                ArcaneError error;
                if (Arcane.TryInit(out error))
                {
                    ArcaneLog.Info("Initialised for game " + Arcane.GameId + " as " + Arcane.UserId + ".");
                }
                else
                {
                    InitializationError = error;
                    ArcaneLog.Warning("Not initialised — " + error + ". Read ArcaneRuntime.InitializationError.");
                }

                AutoInitializeRan = true;
            }

            Ensure();
        }

        private void Awake()
        {
            if (_instance != null && _instance != this)
            {
                Destroy(gameObject);
                return;
            }

            _instance = this;
            Arcane.MainThreadPumpActive = true;
        }

        private void Update()
        {
            ArcaneSettings settings = ArcaneSettings.Instance;

            if (settings.CountFrames)
            {
                Arcane.Frame();
            }

            Arcane.DrainMainThreadWork();

            _sinceLastPoll += Time.unscaledDeltaTime;
            if (_sinceLastPoll < settings.LobbyPollInterval)
            {
                return;
            }

            _sinceLastPoll = 0f;

            if (settings.PumpLobbyEvents)
            {
                Arcane.Lobbies.PumpEvents();
            }

            if (settings.ReportGraphicsSettings)
            {
                ReportGraphicsIfChanged();
            }
        }

        private void OnApplicationQuit()
        {
            if (ArcaneSettings.Instance.ShutdownOnQuit)
            {
                Arcane.Shutdown();
            }
        }

        private void OnDestroy()
        {
            if (_instance != this)
            {
                return;
            }

            _instance = null;
            Arcane.MainThreadPumpActive = false;
        }

        /// <summary>
        /// Tell the SDK what the player is running, so the FPS samples that
        /// follow carry it.
        /// </summary>
        /// <remarks>
        /// Only on a change: the call takes a short lock, and the value only
        /// matters when it differs from the last one reported.
        /// </remarks>
        private void ReportGraphicsIfChanged()
        {
            if (!Arcane.IsInitialized)
            {
                return;
            }

            string resolution = Screen.width + "x" + Screen.height;
            string preset = CurrentQualityPreset();

            if (resolution == _reportedResolution && preset == _reportedPreset)
            {
                return;
            }

            ArcaneError error;
            if (Arcane.TrySetGraphics(resolution, preset, out error))
            {
                _reportedResolution = resolution;
                _reportedPreset = preset;
            }
        }

        private static string CurrentQualityPreset()
        {
            string[] names = QualitySettings.names;
            int level = QualitySettings.GetQualityLevel();
            return level >= 0 && level < names.Length ? names[level] : string.Empty;
        }
    }
}
#endif
