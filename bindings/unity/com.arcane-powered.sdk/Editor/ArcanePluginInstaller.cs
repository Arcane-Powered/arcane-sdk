// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.IO;
using System.IO.Compression;
using System.Threading;
using UnityEditor;
using UnityEngine;
using UnityEngine.Networking;

namespace ArcanePowered.Editor
{
    /// <summary>
    /// <b>Tools ▸ Arcane Powered ▸ Install native plugins</b>: downloads the
    /// Windows, macOS and Linux libraries built for this package version from
    /// its GitHub release, so no Rust toolchain is needed.
    /// </summary>
    internal static class ArcanePluginInstaller
    {
        private const string ReleaseUrl =
            "https://github.com/Arcane-Powered/arcane-sdk/releases/download/v{0}/arcane-sdk-unity-plugins.zip";

        private const string PluginsFolder = "Assets/Plugins/Arcane";
        private const int DownloadTimeoutSeconds = 120;

        [MenuItem("Tools/Arcane Powered/Install native plugins", priority = 10)]
        internal static void Install()
        {
            string version = PackageVersion();
            if (string.IsNullOrEmpty(version))
            {
                Debug.LogError("[Arcane] Could not read the package version; install the plugins with build-plugins.sh.");
                return;
            }

            string url = string.Format(ReleaseUrl, version);
            byte[] archive;
            using (UnityWebRequest request = UnityWebRequest.Get(url))
            {
                request.timeout = DownloadTimeoutSeconds;
                UnityWebRequestAsyncOperation operation = request.SendWebRequest();
                try
                {
                    while (!operation.isDone)
                    {
                        if (EditorUtility.DisplayCancelableProgressBar(
                                "Arcane Powered", "Downloading the native plugins " + version + "…",
                                request.downloadProgress))
                        {
                            request.Abort();
                            return;
                        }

                        Thread.Sleep(50);
                    }
                }
                finally
                {
                    EditorUtility.ClearProgressBar();
                }

                if (request.result != UnityWebRequest.Result.Success)
                {
                    Debug.LogError("[Arcane] Download failed from " + url + ": " + request.error);
                    return;
                }

                archive = request.downloadHandler.data;
            }

            string projectRoot = Path.GetDirectoryName(Application.dataPath);
            int written = Extract(archive, projectRoot);

            AssetDatabase.Refresh();
            Debug.Log("[Arcane] Installed " + written + " native plugin(s) " + version + " into " + PluginsFolder +
                      ". Restart the Editor if a previous plugin was already loaded.");
        }

        internal static int Extract(byte[] archive, string projectRoot)
        {
            string allowedRoot = Path.GetFullPath(Path.Combine(projectRoot, PluginsFolder)) + Path.DirectorySeparatorChar;
            int written = 0;

            using (var stream = new MemoryStream(archive))
            using (var zip = new ZipArchive(stream, ZipArchiveMode.Read))
            {
                foreach (ZipArchiveEntry entry in zip.Entries)
                {
                    if (string.IsNullOrEmpty(entry.Name))
                    {
                        continue;
                    }

                    string destination = Path.GetFullPath(Path.Combine(projectRoot, entry.FullName));
                    if (!destination.StartsWith(allowedRoot, StringComparison.Ordinal) ||
                        !ArcanePluginImporter.IsArcanePlugin(entry.FullName))
                    {
                        continue;
                    }

                    Directory.CreateDirectory(Path.GetDirectoryName(destination));
                    using (Stream source = entry.Open())
                    using (FileStream target = File.Create(destination))
                    {
                        source.CopyTo(target);
                    }

                    written++;
                }
            }

            return written;
        }

        private static string PackageVersion()
        {
            var info = UnityEditor.PackageManager.PackageInfo.FindForAssetPath("Packages/com.arcane-powered.sdk");
            return info == null ? null : info.version;
        }
    }
}
