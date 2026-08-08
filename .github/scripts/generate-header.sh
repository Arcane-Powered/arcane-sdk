#!/usr/bin/env bash
# Generate include/arcane_sdk.h from the Rust FFI surface.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if ! command -v cbindgen >/dev/null 2>&1; then
  echo "cbindgen not found; install with: cargo install cbindgen" >&2
  exit 1
fi

mkdir -p include
cbindgen --config cbindgen.toml --crate arcane-sdk --output include/arcane_sdk.h

# cbindgen sometimes prefixes declarations with a stray space after block comments.
tmp="$(mktemp)"
sed -E 's/^ int /int /g' include/arcane_sdk.h >"$tmp"
mv "$tmp" include/arcane_sdk.h

echo "Wrote include/arcane_sdk.h"
