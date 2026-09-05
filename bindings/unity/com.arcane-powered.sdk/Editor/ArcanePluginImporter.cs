// SPDX-License-Identifier: MIT OR Apache-2.0
using System.IO;
using UnityEditor;
using UnityEngine;

namespace ArcanePowered.Editor
{
    /// <summary>
    /// Points each imported <c>arcane_sdk</c> binary at the platform it was built
    /// for.
    /// </summary>
    /// <remarks>
    /// Unity's default for a freshly dropped native plugin is "compatible with
    /// any platform", which puts a Windows DLL in a Linux build and fails at
    /// load time. The extension says which platform a build is for, so the
    /// import settings can too — and the settings are only touched when they are
    /// wrong, so a deliberate change of yours survives the next reimport.
    /// </remarks>
    internal sealed class ArcanePluginImporter : AssetPostprocessor
    {
        private const string PluginName = "arcane_sdk";

        private static void OnPostprocessAllAssets(
            string[] importedAssets,
            string[] deletedAssets,
            string[] movedAssets,
            string[] movedFromAssetPaths)
        {
            foreach (string path in importedAssets)
            {
                if (IsArcanePlugin(path))
                {
                    Configure(path);
                }
            }
        }

        /// <summary>Whether this asset is one of our native libraries.</summary>
        internal static bool IsArcanePlugin(string assetPath)
        {
            string name = Path.GetFileNameWithoutExtension(assetPath);
            if (name != PluginName && name != "lib" + PluginName)
            {
                return false;
            }

            switch (Path.GetExtension(assetPath).ToLowerInvariant())
            {
                case ".dll":
                case ".so":
                case ".dylib":
                case ".bundle":
                    return true;
                default:
                    return false;
            }
        }

        private static void Configure(string assetPath)
        {
            var importer = AssetImporter.GetAtPath(assetPath) as PluginImporter;
            if (importer == null)
            {
                return;
            }

            BuildTarget target;
            string editorOs;
            string editorCpu;
            switch (Path.GetExtension(assetPath).ToLowerInvariant())
            {
                case ".dll":
                    target = BuildTarget.StandaloneWindows64;
                    editorOs = "Windows";
                    editorCpu = "x86_64";
                    break;
                case ".so":
                    target = BuildTarget.StandaloneLinux64;
                    editorOs = "Linux";
                    editorCpu = "x86_64";
                    break;
                default:
                    target = BuildTarget.StandaloneOSX;
                    editorOs = "OSX";
                    // macOS ships as a universal binary, and an Apple Silicon
                    // Editor must not be told the library is Intel-only.
                    editorCpu = "AnyCPU";
                    break;
            }

            if (importer.GetCompatibleWithAnyPlatform() == false &&
                importer.GetCompatibleWithPlatform(target) &&
                importer.GetCompatibleWithEditor())
            {
                return;
            }

            importer.SetCompatibleWithAnyPlatform(false);
            importer.SetCompatibleWithPlatform(target, true);
            importer.SetCompatibleWithEditor(true);
            importer.SetEditorData("OS", editorOs);
            importer.SetEditorData("CPU", editorCpu);
            importer.SaveAndReimport();

            Debug.Log("[Arcane] Configured " + assetPath + " for " + target + " and the Editor.");
        }
    }
}
