// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Text;

namespace ArcanePowered.Native
{
    /// <summary>
    /// Buffer plumbing for the C ABI: UTF-8 in, UTF-8 out, and the retry that
    /// turns <c>ARCANE_ERR_BUFFER_TOO_SMALL</c> into a bigger buffer instead of
    /// a failure the caller has to think about.
    /// </summary>
    internal static class ArcaneBuffer
    {
        /// <summary>Signature shared by every <c>char *buf, size_t len</c> getter.</summary>
        internal delegate int Getter(byte[] buffer, UIntPtr length);

        /// <summary>Buffer for an <c>err_buf</c> out-parameter. The SDK truncates rather than failing.</summary>
        internal const int ErrorBufferSize = 512;

        /// <summary>Enough for a user id, a game id or a device hash.</summary>
        internal const int IdBufferSize = 64;

        /// <summary>Enough for the session JSON, which the C ABI documents as fitting in 256 bytes.</summary>
        internal const int SessionBufferSize = 256;

        /// <summary>Enough for the error JSON, hint and context included.</summary>
        internal const int ErrorJsonBufferSize = 1024;

        /// <summary>Starting size for the list JSON getters, which grow from here.</summary>
        internal const int ListBufferSize = 8192;

        /// <summary>Ceiling for the growth loop: past this a title's list is a bug, not a big title.</summary>
        internal const int MaxBufferSize = 4 * 1024 * 1024;

        private static readonly UTF8Encoding Utf8 = new UTF8Encoding(false, false);

        /// <summary>
        /// NUL-terminated UTF-8 for a <c>const char *</c> parameter.
        /// <see langword="null"/> stays null — the SDK reads it as "no value".
        /// </summary>
        internal static byte[] ToUtf8(string value)
        {
            if (value == null)
            {
                return null;
            }

            int count = Utf8.GetByteCount(value);
            var bytes = new byte[count + 1];
            Utf8.GetBytes(value, 0, value.Length, bytes, 0);
            bytes[count] = 0;
            return bytes;
        }

        /// <summary>
        /// Run a getter, growing the buffer while it answers
        /// <c>ARCANE_ERR_BUFFER_TOO_SMALL</c>.
        /// </summary>
        /// <remarks>
        /// The SDK never truncates: a short buffer leaves an empty string and
        /// keeps whatever it was about to hand over — lobby events included —
        /// so retrying with twice the room loses nothing.
        /// </remarks>
        /// <returns>The bytes written, or the negative code the getter returned.</returns>
        internal static int Read(Getter getter, int initialSize, out string value)
        {
            value = null;
            int size = Math.Max(initialSize, 8);

            while (true)
            {
                var buffer = new byte[size];
                int written = getter(buffer, (UIntPtr)buffer.Length);

                if (written >= 0)
                {
                    value = Utf8.GetString(buffer, 0, Math.Min(written, buffer.Length));
                    return written;
                }

                if (written != ArcaneNative.ErrBufferTooSmall || size >= MaxBufferSize)
                {
                    return written;
                }

                size = Math.Min(size * 2, MaxBufferSize);
            }
        }

        /// <summary>Allocate an <c>err_buf</c> for an action call.</summary>
        internal static byte[] ErrorBuffer()
        {
            return new byte[ErrorBufferSize];
        }

        /// <summary>
        /// Decode an <c>err_buf</c>, stopping at the NUL the SDK always writes.
        /// </summary>
        internal static string DecodeCString(byte[] buffer)
        {
            if (buffer == null)
            {
                return string.Empty;
            }

            int end = Array.IndexOf(buffer, (byte)0);
            if (end < 0)
            {
                end = buffer.Length;
            }

            return Utf8.GetString(buffer, 0, end);
        }
    }
}
