#!/usr/bin/env bash
# PR check: releasing titles must bump Cargo.toml; others must leave it unchanged.
# Usage: check-version-bump.sh "<pr title>" <base-ref>
# Example: check-version-bump.sh "feat: add tickets" origin/main
set -euo pipefail

TITLE="${1:-}"
BASE_REF="${2:-}"
if [[ -z "$TITLE" || -z "$BASE_REF" ]]; then
  echo "Usage: $0 \"<pr title>\" <base-ref>" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=/dev/null
source "${ROOT}/.github/scripts/semver.sh"

HEAD_VERSION="$(semver_read_cargo_version "${ROOT}/Cargo.toml")"

tmp="$(mktemp)"
git show "${BASE_REF}:Cargo.toml" >"$tmp"
BASE_VERSION="$(semver_read_cargo_version "$tmp")"
rm -f "$tmp"

BUMP="$(semver_parse_bump "$TITLE")"
if [[ "$BUMP" == "invalid" ]]; then
  echo "Title is not conventional: $TITLE" >&2
  exit 1
fi

if [[ "$BUMP" == "none" ]]; then
  if [[ "$HEAD_VERSION" != "$BASE_VERSION" ]]; then
    echo "PR title does not release, but Cargo.toml changed: ${BASE_VERSION} → ${HEAD_VERSION}" >&2
    echo "Keep version at ${BASE_VERSION}, or use feat/fix/perf/breaking title." >&2
    exit 1
  fi
  echo "No release (${TITLE}); version unchanged at ${HEAD_VERSION}."
  exit 0
fi

EXPECTED="$(semver_bump_version "$BASE_VERSION" "$BUMP")"
if [[ "$HEAD_VERSION" != "$EXPECTED" ]]; then
  echo "Cargo.toml version must be ${EXPECTED} for this PR (base ${BASE_VERSION}, bump ${BUMP})." >&2
  echo "Found ${HEAD_VERSION}. Update Cargo.toml (and Cargo.lock), e.g.:" >&2
  echo "  .github/scripts/bump-version.sh $(printf '%q' "$TITLE")" >&2
  exit 1
fi

echo "OK: ${BASE_VERSION} → ${HEAD_VERSION} (${BUMP}) for: ${TITLE}"
