// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using ArcanePowered.Json;

namespace ArcanePowered.Native
{
    /// <summary>
    /// The two C ABI return conventions, once each, so no call site has to
    /// remember what <c>-3</c> means.
    /// </summary>
    internal static class ArcaneCall
    {
        /// <summary>An action taking an <c>err_buf</c>: <c>0</c>, <c>1</c>, or <c>2</c>.</summary>
        internal delegate int Action(byte[] errorBuffer, UIntPtr errorLength);

        /// <summary>
        /// Run an action, mapping its return code to an
        /// <see cref="ArcaneError"/>.
        /// </summary>
        internal static bool Run(Action action, out ArcaneError error)
        {
            if (!ArcaneNative.IsAvailable)
            {
                error = ArcaneError.PluginMissing();
                return false;
            }

            var buffer = ArcaneBuffer.ErrorBuffer();
            int rc = action(buffer, (UIntPtr)buffer.Length);

            if (rc == ArcaneNative.Ok)
            {
                error = null;
                return true;
            }

            if (rc == ArcaneNative.ErrArgument)
            {
                // The SDK spends one code on "bad argument or no client"; the
                // singleton itself can tell the two apart.
                error = ArcaneNative.arcane_sdk_is_initialized() == 1
                    ? ArcaneError.Argument(
                        "The Arcane SDK rejected an argument.",
                        "Check the ids and keys you passed — they must be non-empty UTF-8.")
                    : ArcaneError.NotInitialized();
                return false;
            }

            error = ArcaneError.Capture(buffer);
            return false;
        }

        /// <summary>
        /// Read a getter that answers with a value, where <c>-4</c> means "no
        /// value" rather than a failure.
        /// </summary>
        /// <returns>The value, or <see langword="null"/> when there is none.</returns>
        internal static string ReadValue(ArcaneBuffer.Getter getter, int initialSize)
        {
            if (!ArcaneNative.IsAvailable)
            {
                return null;
            }

            string value;
            return ArcaneBuffer.Read(getter, initialSize, out value) < 0 ? null : value;
        }

        /// <summary>
        /// Read a getter that makes a call, where <c>-4</c> means the call
        /// failed and the reason is in the SDK's last-error record.
        /// </summary>
        internal static bool ReadJson(
            ArcaneBuffer.Getter getter,
            int initialSize,
            out ArcaneJson root,
            out ArcaneError error)
        {
            root = null;

            if (!ArcaneNative.IsAvailable)
            {
                error = ArcaneError.PluginMissing();
                return false;
            }

            string json;
            int written = ArcaneBuffer.Read(getter, initialSize, out json);

            if (written < 0)
            {
                error = Describe(written);
                return false;
            }

            if (!ArcaneJson.TryParse(json, out root) || root.Kind != ArcaneJsonKind.Object)
            {
                error = ArcaneError.InvalidResponse("The Arcane SDK answered with a document this package cannot read.");
                return false;
            }

            error = null;
            return true;
        }

        /// <summary>Turn a negative getter code into an error.</summary>
        private static ArcaneError Describe(int code)
        {
            switch (code)
            {
                case ArcaneNative.ErrNotInitialized:
                    return ArcaneError.NotInitialized();

                case ArcaneNative.ErrBadBuffer:
                    // Our buffers are never null, so this is the SDK refusing an
                    // argument: a bad join code, a payload that is not base64,
                    // an unknown visibility.
                    return ArcaneError.FromLastError() ?? ArcaneError.Argument(
                        "The Arcane SDK rejected an argument.",
                        "Check the ids, join code and payload you passed.");

                case ArcaneNative.ErrBufferTooSmall:
                    return ArcaneError.InvalidResponse(
                        "The Arcane SDK answer did not fit in " + (ArcaneBuffer.MaxBufferSize / 1024) + " KiB.");

                default:
                    return ArcaneError.Capture(null);
            }
        }
    }
}
