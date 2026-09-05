// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Text;
using ArcanePowered.Native;
using NUnit.Framework;

namespace ArcanePowered.Tests
{
    /// <summary>
    /// The marshalling layer, driven by a stand-in getter that behaves the way
    /// the C ABI documents — so the growth loop is exercised without needing the
    /// Arcane desktop app.
    /// </summary>
    public sealed class ArcaneBufferTests
    {
        [Test]
        public void GrowsTheBufferUntilTheAnswerFits()
        {
            byte[] answer = Encoding.UTF8.GetBytes(new string('x', 5000));
            int attempts = 0;

            string value;
            int written = ArcaneBuffer.Read(
                (buffer, length) =>
                {
                    attempts++;
                    if ((int)length < answer.Length + 1)
                    {
                        buffer[0] = 0;  // the SDK empties the buffer, never truncates
                        return -3;
                    }

                    Array.Copy(answer, buffer, answer.Length);
                    buffer[answer.Length] = 0;
                    return answer.Length;
                },
                64,
                out value);

            Assert.AreEqual(5000, written);
            Assert.AreEqual(5000, value.Length);
            Assert.Greater(attempts, 1, "a 64-byte buffer cannot have held 5000 bytes on the first try");
        }

        [Test]
        public void DoesNotRetryARealFailure()
        {
            string value;

            Assert.AreEqual(-1, ArcaneBuffer.Read((buffer, length) => -1, 64, out value));
            Assert.IsNull(value);
        }

        [Test]
        public void EncodesStringsAsNulTerminatedUtf8()
        {
            byte[] encoded = ArcaneBuffer.ToUtf8("é");

            Assert.AreEqual(3, encoded.Length, "two bytes for the character, one for the NUL");
            Assert.AreEqual(0, encoded[2]);
        }

        [Test]
        public void ANullStringStaysANullPointer()
        {
            Assert.IsNull(ArcaneBuffer.ToUtf8(null), "the C ABI reads a null pointer as 'no value'");
        }

        [Test]
        public void DecodingStopsAtTheNul()
        {
            Assert.AreEqual("hi", ArcaneBuffer.DecodeCString(new byte[] { 104, 105, 0, 120 }));
            Assert.AreEqual(string.Empty, ArcaneBuffer.DecodeCString(new byte[] { 0 }));
            Assert.AreEqual(string.Empty, ArcaneBuffer.DecodeCString(null));
        }
    }
}
