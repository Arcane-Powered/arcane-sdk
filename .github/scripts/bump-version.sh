#!/usr/bin/env bash
# Bump SemVer from a Conventional Commit / PR title.
# Usage: bump-version.sh "<pr title>"
# Prints: BUMP=<none|patch|minor|major> VERSION=<x.y.z>
set -euo pipefail

TITLE="${1:-}"
if [[ -z "$TITLE" ]]; then
  echo "Usage: $0 \"<conventional pr title>\"" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CARGO_TOML="${ROOT}/Cargo.toml"

if [[ ! -f "$CARGO_TOML" ]]; then
  echo "Cargo.toml not found at $CARGO_TOML" >&2
  exit 1
fi

CURRENT="$(
  grep -E '^version[[:space:]]*=' "$CARGO_TOML" | head -1 \
    | sed -E 's/^version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/'
)"
if [[ ! "$CURRENT" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Invalid version in Cargo.toml: '$CURRENT'" >&2
  exit 1
fi

IFS=. read -r MAJOR MINOR PATCH <<<"$CURRENT"

# type[!](scope)?: subject  OR  type!: subject
TYPE="$(printf '%s' "$TITLE" | sed -nE 's/^([a-z]+)(\([^)]*\))?(!)?:.*/\1/p')"
BREAKING="$(printf '%s' "$TITLE" | sed -nE 's/^[a-z]+(\([^)]*\))?(!)?:.*/\2/p')"

BUMP="none"
if [[ -n "$BREAKING" ]] || printf '%s' "$TITLE" | grep -q 'BREAKING CHANGE'; then
  BUMP="major"
elif [[ "$TYPE" == "feat" ]]; then
  BUMP="minor"
elif [[ "$TYPE" == "fix" || "$TYPE" == "perf" ]]; then
  BUMP="patch"
elif [[ -z "$TYPE" ]]; then
  echo "Title is not conventional: $TITLE" >&2
  echo "BUMP=none"
  echo "VERSION=$CURRENT"
  exit 0
else
  # chore/docs/ci/refactor/test/build/revert → no release
  echo "BUMP=none"
  echo "VERSION=$CURRENT"
  exit 0
fi

case "$BUMP" in
  major)
    MAJOR=$((MAJOR + 1))
    MINOR=0
    PATCH=0
    ;;
  minor)
    MINOR=$((MINOR + 1))
    PATCH=0
    ;;
  patch)
    PATCH=$((PATCH + 1))
    ;;
esac

NEW="${MAJOR}.${MINOR}.${PATCH}"

tmp="$(mktemp)"
sed -E "s/^version = \"${CURRENT}\"/version = \"${NEW}\"/" "$CARGO_TOML" >"$tmp"
mv "$tmp" "$CARGO_TOML"

# Keep the root package entry in Cargo.lock in sync when present
CARGO_LOCK="${ROOT}/Cargo.lock"
if [[ -f "$CARGO_LOCK" ]]; then
  python3 - "$CARGO_LOCK" "$CURRENT" "$NEW" <<'PY'
import pathlib, sys
path, old, new = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
text = path.read_text()
needle = f'name = "arcane-sdk"\nversion = "{old}"'
repl = f'name = "arcane-sdk"\nversion = "{new}"'
if needle not in text:
    sys.exit(0)
path.write_text(text.replace(needle, repl, 1))
PY
fi

echo "BUMP=$BUMP"
echo "VERSION=$NEW"
