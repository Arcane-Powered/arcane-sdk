// SPDX-License-Identifier: MIT OR Apache-2.0
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;

namespace ArcanePowered.Json
{
    /// <summary>What a <see cref="ArcaneJson"/> node holds.</summary>
    internal enum ArcaneJsonKind
    {
        Null,
        Bool,
        Number,
        String,
        Array,
        Object,
    }

    /// <summary>
    /// A tiny read-only JSON tree, enough for the documents the C ABI writes.
    /// </summary>
    /// <remarks>
    /// Unity's <c>JsonUtility</c> cannot express what these documents need — a
    /// <c>null</c> <c>unlocked_at</c> that is not zero, a <c>join_code</c> that
    /// is absent rather than empty, events discriminated by a <c>type</c> field
    /// — and pulling in a third-party serializer would put a dependency in
    /// front of the SDK. Missing members read back as
    /// <see cref="ArcaneJsonKind.Null"/> instead of throwing, so a document from
    /// an older desktop app degrades to defaults rather than an exception.
    /// </remarks>
    internal sealed class ArcaneJson
    {
        private static readonly ArcaneJson NullNode = new ArcaneJson(ArcaneJsonKind.Null);
        private static readonly List<ArcaneJson> NoItems = new List<ArcaneJson>();

        private readonly ArcaneJsonKind _kind;
        private readonly bool _bool;
        private readonly string _text;
        private readonly List<ArcaneJson> _items;
        private readonly Dictionary<string, ArcaneJson> _members;

        private ArcaneJson(ArcaneJsonKind kind)
        {
            _kind = kind;
        }

        private ArcaneJson(bool value)
        {
            _kind = ArcaneJsonKind.Bool;
            _bool = value;
        }

        private ArcaneJson(ArcaneJsonKind kind, string text)
        {
            _kind = kind;
            _text = text;
        }

        private ArcaneJson(List<ArcaneJson> items)
        {
            _kind = ArcaneJsonKind.Array;
            _items = items;
        }

        private ArcaneJson(Dictionary<string, ArcaneJson> members)
        {
            _kind = ArcaneJsonKind.Object;
            _members = members;
        }

        internal ArcaneJsonKind Kind
        {
            get { return _kind; }
        }

        internal bool IsNull
        {
            get { return _kind == ArcaneJsonKind.Null; }
        }

        /// <summary>Members of an object, or an empty list for anything else.</summary>
        internal IList<ArcaneJson> Items
        {
            get { return _items ?? NoItems; }
        }

        /// <summary>An object member, or a null node when it is absent.</summary>
        internal ArcaneJson this[string key]
        {
            get
            {
                ArcaneJson value;
                if (_members != null && _members.TryGetValue(key, out value))
                {
                    return value;
                }

                return NullNode;
            }
        }

        /// <summary>The string value, or <paramref name="fallback"/> for any other kind.</summary>
        internal string AsString(string fallback = null)
        {
            return _kind == ArcaneJsonKind.String ? _text : fallback;
        }

        /// <summary>The boolean value, or <paramref name="fallback"/> for any other kind.</summary>
        internal bool AsBool(bool fallback = false)
        {
            return _kind == ArcaneJsonKind.Bool ? _bool : fallback;
        }

        /// <summary>The number as a 64-bit integer, or <paramref name="fallback"/>.</summary>
        internal long AsLong(long fallback = 0)
        {
            long parsed;
            if (_kind == ArcaneJsonKind.Number &&
                long.TryParse(_text, NumberStyles.Integer, CultureInfo.InvariantCulture, out parsed))
            {
                return parsed;
            }

            double asDouble;
            if (_kind == ArcaneJsonKind.Number &&
                double.TryParse(_text, NumberStyles.Float, CultureInfo.InvariantCulture, out asDouble))
            {
                return (long)asDouble;
            }

            return fallback;
        }

        /// <summary>The number as a 64-bit integer, or <see langword="null"/> when it is JSON null or absent.</summary>
        internal long? AsNullableLong()
        {
            return _kind == ArcaneJsonKind.Number ? AsLong() : (long?)null;
        }

        /// <summary>The number as a float, or <see langword="null"/> when it is JSON null or absent.</summary>
        internal float? AsNullableFloat()
        {
            double parsed;
            if (_kind == ArcaneJsonKind.Number &&
                double.TryParse(_text, NumberStyles.Float, CultureInfo.InvariantCulture, out parsed))
            {
                return (float)parsed;
            }

            return null;
        }

        /// <summary>Object members as key/value pairs, empty for any other kind.</summary>
        internal IEnumerable<KeyValuePair<string, ArcaneJson>> Members()
        {
            if (_members == null)
            {
                yield break;
            }

            foreach (var member in _members)
            {
                yield return member;
            }
        }

        /// <summary>Parse a document, or throw <see cref="FormatException"/>.</summary>
        internal static ArcaneJson Parse(string text)
        {
            if (text == null)
            {
                throw new FormatException("Cannot parse a null JSON document.");
            }

            int index = 0;
            SkipWhitespace(text, ref index);
            ArcaneJson value = ParseValue(text, ref index);
            SkipWhitespace(text, ref index);
            if (index != text.Length)
            {
                throw new FormatException("Trailing content after the JSON value at index " + index + ".");
            }

            return value;
        }

        /// <summary>Parse a document, answering <see langword="false"/> instead of throwing.</summary>
        internal static bool TryParse(string text, out ArcaneJson value)
        {
            try
            {
                value = Parse(text);
                return true;
            }
            catch (FormatException)
            {
                value = NullNode;
                return false;
            }
        }

        private static ArcaneJson ParseValue(string text, ref int index)
        {
            if (index >= text.Length)
            {
                throw new FormatException("Unexpected end of JSON document.");
            }

            char c = text[index];
            switch (c)
            {
                case '{':
                    return ParseObject(text, ref index);
                case '[':
                    return ParseArray(text, ref index);
                case '"':
                    return new ArcaneJson(ArcaneJsonKind.String, ParseString(text, ref index));
                case 't':
                    Expect(text, ref index, "true");
                    return new ArcaneJson(true);
                case 'f':
                    Expect(text, ref index, "false");
                    return new ArcaneJson(false);
                case 'n':
                    Expect(text, ref index, "null");
                    return NullNode;
                default:
                    return new ArcaneJson(ArcaneJsonKind.Number, ParseNumber(text, ref index));
            }
        }

        private static ArcaneJson ParseObject(string text, ref int index)
        {
            index++; // '{'
            var members = new Dictionary<string, ArcaneJson>(StringComparer.Ordinal);
            SkipWhitespace(text, ref index);

            if (index < text.Length && text[index] == '}')
            {
                index++;
                return new ArcaneJson(members);
            }

            while (true)
            {
                SkipWhitespace(text, ref index);
                if (index >= text.Length || text[index] != '"')
                {
                    throw new FormatException("Expected a member name at index " + index + ".");
                }

                string key = ParseString(text, ref index);
                SkipWhitespace(text, ref index);
                if (index >= text.Length || text[index] != ':')
                {
                    throw new FormatException("Expected ':' at index " + index + ".");
                }

                index++;
                SkipWhitespace(text, ref index);
                members[key] = ParseValue(text, ref index);
                SkipWhitespace(text, ref index);

                if (index >= text.Length)
                {
                    throw new FormatException("Unterminated JSON object.");
                }

                if (text[index] == ',')
                {
                    index++;
                    continue;
                }

                if (text[index] == '}')
                {
                    index++;
                    return new ArcaneJson(members);
                }

                throw new FormatException("Expected ',' or '}' at index " + index + ".");
            }
        }

        private static ArcaneJson ParseArray(string text, ref int index)
        {
            index++; // '['
            var items = new List<ArcaneJson>();
            SkipWhitespace(text, ref index);

            if (index < text.Length && text[index] == ']')
            {
                index++;
                return new ArcaneJson(items);
            }

            while (true)
            {
                SkipWhitespace(text, ref index);
                items.Add(ParseValue(text, ref index));
                SkipWhitespace(text, ref index);

                if (index >= text.Length)
                {
                    throw new FormatException("Unterminated JSON array.");
                }

                if (text[index] == ',')
                {
                    index++;
                    continue;
                }

                if (text[index] == ']')
                {
                    index++;
                    return new ArcaneJson(items);
                }

                throw new FormatException("Expected ',' or ']' at index " + index + ".");
            }
        }

        private static string ParseString(string text, ref int index)
        {
            index++; // opening quote
            var builder = new StringBuilder();

            while (true)
            {
                if (index >= text.Length)
                {
                    throw new FormatException("Unterminated JSON string.");
                }

                char c = text[index++];
                if (c == '"')
                {
                    return builder.ToString();
                }

                if (c != '\\')
                {
                    builder.Append(c);
                    continue;
                }

                if (index >= text.Length)
                {
                    throw new FormatException("Unterminated JSON escape.");
                }

                char escape = text[index++];
                switch (escape)
                {
                    case '"': builder.Append('"'); break;
                    case '\\': builder.Append('\\'); break;
                    case '/': builder.Append('/'); break;
                    case 'b': builder.Append('\b'); break;
                    case 'f': builder.Append('\f'); break;
                    case 'n': builder.Append('\n'); break;
                    case 'r': builder.Append('\r'); break;
                    case 't': builder.Append('\t'); break;
                    case 'u':
                        if (index + 4 > text.Length)
                        {
                            throw new FormatException("Truncated \\u escape at index " + index + ".");
                        }

                        int code;
                        if (!int.TryParse(
                                text.Substring(index, 4),
                                NumberStyles.HexNumber,
                                CultureInfo.InvariantCulture,
                                out code))
                        {
                            throw new FormatException("Malformed \\u escape at index " + index + ".");
                        }

                        // Surrogate pairs arrive as two escapes; appending each
                        // half in turn rebuilds the astral character.
                        builder.Append((char)code);
                        index += 4;
                        break;
                    default:
                        throw new FormatException("Unknown escape '\\" + escape + "' at index " + (index - 1) + ".");
                }
            }
        }

        private static string ParseNumber(string text, ref int index)
        {
            int start = index;

            if (index < text.Length && (text[index] == '-' || text[index] == '+'))
            {
                index++;
            }

            while (index < text.Length)
            {
                char c = text[index];
                if ((c >= '0' && c <= '9') || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
                {
                    index++;
                    continue;
                }

                break;
            }

            if (index == start)
            {
                throw new FormatException("Expected a JSON value at index " + start + ".");
            }

            return text.Substring(start, index - start);
        }

        private static void Expect(string text, ref int index, string literal)
        {
            if (index + literal.Length > text.Length ||
                string.CompareOrdinal(text, index, literal, 0, literal.Length) != 0)
            {
                throw new FormatException("Expected '" + literal + "' at index " + index + ".");
            }

            index += literal.Length;
        }

        private static void SkipWhitespace(string text, ref int index)
        {
            while (index < text.Length)
            {
                char c = text[index];
                if (c == ' ' || c == '\t' || c == '\n' || c == '\r')
                {
                    index++;
                    continue;
                }

                break;
            }
        }
    }
}
