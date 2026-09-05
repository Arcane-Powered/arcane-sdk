// SPDX-License-Identifier: MIT OR Apache-2.0
using System.Runtime.CompilerServices;

// The JSON reader and the marshalling helpers are internal on purpose — they are
// plumbing, not API — but they are also the parts most worth testing.
[assembly: InternalsVisibleTo("ArcanePowered.Editor")]
[assembly: InternalsVisibleTo("ArcanePowered.Tests")]
