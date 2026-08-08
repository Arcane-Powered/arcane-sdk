#!/usr/bin/env bash
# Shared SemVer helpers for Conventional Commit / PR titles.
# shellcheck shell=bash

semver_parse_bump() {
  local title="$1"
  local type breaking

  type="$(printf '%s' "$title" | sed -nE 's/^([a-z]+)(\([^)]*\))?(!)?:.*/\1/p')"
  breaking="$(printf '%s' "$title" | sed -nE 's/^[a-z]+(\([^)]*\))?(!)?:.*/\2/p')"

  if [[ -n "$breaking" ]] || printf '%s' "$title" | grep -q 'BREAKING CHANGE'; then
    printf '%s\n' "major"
  elif [[ "$type" == "feat" ]]; then
    printf '%s\n' "minor"
  elif [[ "$type" == "fix" || "$type" == "perf" ]]; then
    printf '%s\n' "patch"
  elif [[ -z "$type" ]]; then
    printf '%s\n' "invalid"
  else
    # chore/docs/ci/refactor/test/build/revert → no release
    printf '%s\n' "none"
  fi
}

semver_bump_version() {
  local current="$1"
  local bump="$2"
  local major minor patch

  IFS=. read -r major minor patch <<<"$current"
  case "$bump" in
    major)
      major=$((major + 1))
      minor=0
      patch=0
      ;;
    minor)
      minor=$((minor + 1))
      patch=0
      ;;
    patch)
      patch=$((patch + 1))
      ;;
    *)
      printf '%s\n' "$current"
      return 0
      ;;
  esac
  printf '%s\n' "${major}.${minor}.${patch}"
}

semver_read_cargo_version() {
  local file="$1"
  local version
  version="$(
    grep -E '^version[[:space:]]*=' "$file" | head -1 \
      | sed -E 's/^version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/'
  )"
  if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Invalid version in $file: '$version'" >&2
    return 1
  fi
  printf '%s\n' "$version"
}

semver_write_cargo_version() {
  local file="$1"
  local old="$2"
  local new="$3"
  local tmp
  tmp="$(mktemp)"
  sed -E "s/^version = \"${old}\"/version = \"${new}\"/" "$file" >"$tmp"
  mv "$tmp" "$file"
}

semver_sync_cargo_lock() {
  local lock="$1"
  local old="$2"
  local new="$3"
  [[ -f "$lock" ]] || return 0
  python3 - "$lock" "$old" "$new" <<'PY'
import pathlib, sys
path, old, new = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
text = path.read_text()
needle = f'name = "arcane-sdk"\nversion = "{old}"'
repl = f'name = "arcane-sdk"\nversion = "{new}"'
if needle not in text:
    sys.exit(0)
path.write_text(text.replace(needle, repl, 1))
PY
}
