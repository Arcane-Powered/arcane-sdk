// SPDX-License-Identifier: MIT OR Apache-2.0
using System.Collections.Generic;
using System.IO;
using UnityEditor;
using UnityEngine;

namespace ArcanePowered.Editor
{
    /// <summary>
    /// <b>Project Settings ▸ Arcane Powered</b>: the settings asset, and enough
    /// diagnostics to tell "the plugin is missing" from "the desktop app is not
    /// running".
    /// </summary>
    internal static class ArcaneSettingsProvider
    {
        private const string ResourcesFolder = "Assets/Resources";
        private const string AssetPath = ResourcesFolder + "/" + ArcaneSettings.ResourceName + ".asset";

        private static UnityEditor.Editor _settingsEditor;

        [SettingsProvider]
        public static SettingsProvider Create()
        {
            return new SettingsProvider("Project/Arcane Powered", SettingsScope.Project)
            {
                label = "Arcane Powered",
                guiHandler = search => Draw(),
                keywords = new HashSet<string>
                {
                    "arcane", "drm", "ownership", "achievements", "friends", "lobbies", "playtime",
                },
            };
        }

        /// <summary>Create the settings asset, or select the one that exists.</summary>
        [MenuItem("Tools/Arcane Powered/Settings", priority = 0)]
        private static void OpenSettings()
        {
            SettingsService.OpenProjectSettings("Project/Arcane Powered");
        }

        /// <summary>Write what the SDK can see right now to the console, for a bug report.</summary>
        [MenuItem("Tools/Arcane Powered/Log diagnostics", priority = 20)]
        private static void LogDiagnostics()
        {
            Debug.Log(Diagnostics());
        }

        private static void Draw()
        {
            EditorGUILayout.Space();

            var settings = AssetDatabase.LoadAssetAtPath<ArcaneSettings>(AssetPath);
            if (settings == null)
            {
                EditorGUILayout.HelpBox(
                    "No settings asset yet. The defaults apply: the SDK initialises before the first scene, " +
                    "counts frames, pumps lobby events and shuts down on quit.\n\n" +
                    "Create one to change that, and to set the game id the Editor runs under.",
                    MessageType.Info);

                if (GUILayout.Button("Create settings asset"))
                {
                    CreateSettingsAsset();
                }
            }
            else
            {
                UnityEditor.Editor.CreateCachedEditor(settings, null, ref _settingsEditor);
                _settingsEditor.OnInspectorGUI();
            }

            EditorGUILayout.Space();
            DrawDiagnostics();
        }

        private static void DrawDiagnostics()
        {
            EditorGUILayout.LabelField("Diagnostics", EditorStyles.boldLabel);

            if (!Arcane.IsPluginAvailable)
            {
                EditorGUILayout.HelpBox(
                    "The native plugin is not loaded. Build it with bindings/unity/build-plugins.sh and let " +
                    "Unity import the result, then restart the Editor — a native library is only loaded once " +
                    "per session.",
                    MessageType.Warning);
                return;
            }

            EditorGUILayout.SelectableLabel(Diagnostics(), EditorStyles.textArea, GUILayout.Height(96f));

            if (GUILayout.Button("Log diagnostics"))
            {
                LogDiagnostics();
            }
        }

        private static string Diagnostics()
        {
            if (!Arcane.IsPluginAvailable)
            {
                return "Native plugin: not loaded";
            }

            if (!Arcane.IsInitialized)
            {
                ArcaneError error = ArcaneRuntime.InitializationError ?? Arcane.LastError;
                return "Native plugin: loaded\nClient: not initialised\nLast error: " +
                       (error == null ? "none" : error.ToString());
            }

            ArcaneSessionInfo session = Arcane.Session;
            return "Native plugin: loaded\n" +
                   "Game id: " + Arcane.GameId + "\n" +
                   "User id: " + Arcane.UserId + "\n" +
                   "Ownership: " + Arcane.Ownership + "\n" +
                   "Session: " + session.Tracking + ", " + session.PlayedSeconds + "s played, lobby events " +
                   session.LobbyEvents;
        }

        private static void CreateSettingsAsset()
        {
            if (!Directory.Exists(ResourcesFolder))
            {
                Directory.CreateDirectory(ResourcesFolder);
                AssetDatabase.Refresh();
            }

            var settings = ScriptableObject.CreateInstance<ArcaneSettings>();
            AssetDatabase.CreateAsset(settings, AssetPath);
            AssetDatabase.SaveAssets();
            Selection.activeObject = settings;
        }
    }
}
