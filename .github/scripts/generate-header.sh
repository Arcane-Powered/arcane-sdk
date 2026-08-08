#!/usr/bin/env bash
# Generate include/arcane_sdk.h from the Rust FFI surface.
#
# Requires cbindgen 0.29.4 (pinned; same as .github/workflows/ci.yml).
#   cargo install cbindgen --version 0.29.4 --locked
set -euo pipefail

EXPECTED_CBINDGEN_VERSION="0.29.4"

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if ! command -v cbindgen >/dev/null 2>&1; then
  echo "cbindgen not found; install with:" >&2
  echo "  cargo install cbindgen --version ${EXPECTED_CBINDGEN_VERSION} --locked" >&2
  exit 1
fi

ACTUAL_VERSION="$(cbindgen --version | awk '{print $2}')"
if [[ "$ACTUAL_VERSION" != "$EXPECTED_CBINDGEN_VERSION" ]]; then
  echo "cbindgen ${ACTUAL_VERSION} found; expected ${EXPECTED_CBINDGEN_VERSION}" >&2
  echo "  cargo install cbindgen --version ${EXPECTED_CBINDGEN_VERSION} --locked --force" >&2
  exit 1
fi

mkdir -p include
cbindgen --config cbindgen.toml --crate arcane-sdk --output include/arcane_sdk.h

# cbindgen sometimes prefixes declarations with a stray space after block comments.
tmp="$(mktemp)"
sed -E 's/^ int /int /g' include/arcane_sdk.h >"$tmp"
mv "$tmp" include/arcane_sdk.h

echo "Wrote include/arcane_sdk.h"
