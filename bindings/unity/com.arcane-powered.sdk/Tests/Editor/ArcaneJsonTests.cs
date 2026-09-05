// SPDX-License-Identifier: MIT OR Apache-2.0
using ArcanePowered.Json;
using NUnit.Framework;

namespace ArcanePowered.Tests
{
    /// <summary>
    /// The reader the whole package sits on: what it does with the shapes the
    /// SDK actually writes, and with the ones it must not choke on.
    /// </summary>
    public sealed class ArcaneJsonTests
    {
        [Test]
        public void ReadsTheKindsTheSdkWrites()
        {
            ArcaneJson root = ArcaneJson.Parse(
                "{\"n\":42,\"f\":-2.5,\"s\":\"x\",\"b\":true,\"z\":null,\"a\":[1,2],\"o\":{\"k\":\"v\"}}");

            Assert.AreEqual(42, root["n"].AsLong());
            Assert.AreEqual(-2.5f, root["f"].AsNullableFloat());
            Assert.AreEqual("x", root["s"].AsString());
            Assert.IsTrue(root["b"].AsBool());
            Assert.IsTrue(root["z"].IsNull);
            Assert.AreEqual(2, root["a"].Items.Count);
            Assert.AreEqual("v", root["o"]["k"].AsString());
        }

        [Test]
        public void MissingMembersReadAsNullRatherThanThrowing()
        {
            ArcaneJson root = ArcaneJson.Parse("{}");

            Assert.IsTrue(root["absent"].IsNull);
            Assert.IsTrue(root["absent"]["deeper"].IsNull);
            Assert.AreEqual("fallback", root["absent"].AsString("fallback"));
            Assert.AreEqual(7, root["absent"].AsLong(7));
        }

        [Test]
        public void NullIsNotZero()
        {
            // The distinction the whole achievements screen rests on: a null
            // unlocked_at is locked, not unlocked in 1970.
            Assert.IsNull(ArcaneJson.Parse("{\"unlocked_at\":null}")["unlocked_at"].AsNullableLong());
            Assert.AreEqual(0, ArcaneJson.Parse("{\"unlocked_at\":0}")["unlocked_at"].AsNullableLong());
        }

        [Test]
        public void ReadingAValueAsTheWrongKindFallsBack()
        {
            ArcaneJson root = ArcaneJson.Parse("{\"n\":1}");

            Assert.AreEqual("fallback", root["n"].AsString("fallback"));
            Assert.IsFalse(root["n"].AsBool());
        }

        [Test]
        public void UnescapesStrings()
        {
            ArcaneJson root = ArcaneJson.Parse("{\"s\":\"a\\\"b\\\\c\\nd\\u00e9\"}");

            Assert.AreEqual("a\"b\\c\né", root["s"].AsString());
        }

        [TestCase("{\"a\":}")]
        [TestCase("{\"a\" 1}")]
        [TestCase("[1,2")]
        [TestCase("{} trailing")]
        [TestCase("")]
        public void RejectsMalformedDocuments(string document)
        {
            ArcaneJson parsed;

            Assert.IsFalse(ArcaneJson.TryParse(document, out parsed));
        }
    }
}
