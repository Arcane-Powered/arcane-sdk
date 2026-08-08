#!/usr/bin/env bash
# Bump SemVer in Cargo.toml / Cargo.lock from a Conventional Commit title.
# Usage: bump-version.sh "<pr title>"
# Prints: BUMP=<none|patch|minor|major> VERSION=<x.y.z>
set -euo pipefail

TITLE="${1:-}"
if [[ -z "$TITLE" ]]; then
  echo "Usage: $0 \"<conventional pr title>\"" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=/dev/null
source "${ROOT}/.github/scripts/semver.sh"

CARGO_TOML="${ROOT}/Cargo.toml"
CARGO_LOCK="${ROOT}/Cargo.lock"

CURRENT="$(semver_read_cargo_version "$CARGO_TOML")"
BUMP="$(semver_parse_bump "$TITLE")"

if [[ "$BUMP" == "invalid" ]]; then
  echo "Title is not conventional: $TITLE" >&2
  echo "BUMP=none"
  echo "VERSION=$CURRENT"
  exit 0
fi

if [[ "$BUMP" == "none" ]]; then
  echo "BUMP=none"
  echo "VERSION=$CURRENT"
  exit 0
fi

NEW="$(semver_bump_version "$CURRENT" "$BUMP")"
semver_write_cargo_version "$CARGO_TOML" "$CURRENT" "$NEW"
semver_sync_cargo_lock "$CARGO_LOCK" "$CURRENT" "$NEW"

echo "BUMP=$BUMP"
echo "VERSION=$NEW"
