#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Build the native SDK and drop it into a Unity project as a native plugin.
#
#   bindings/unity/build-plugins.sh ~/games/my-game
#   bindings/unity/build-plugins.sh ~/games/my-game --target x86_64-pc-windows-gnu
#   bindings/unity/build-plugins.sh ~/games/my-game \
#       --target aarch64-apple-darwin --target x86_64-apple-darwin   # universal
#
# Every build is cross-compiled from this machine, so a Windows plugin needs the
# Windows target installed (`rustup target add …`) and a linker for it. The
# Editor loads the plugin for the platform it runs on, so build that one first.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
project=""
targets=()

usage() {
  awk 'NR > 2 && /^#/ { sub(/^# ?/, ""); print; next } NR > 2 { exit }' "${BASH_SOURCE[0]}"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      [[ $# -ge 2 ]] || { echo "error: --target needs a value" >&2; exit 2; }
      targets+=("$2")
      shift 2
      ;;
    -h|--help)
      usage 0
      ;;
    -*)
      echo "error: unknown option $1" >&2
      usage 2
      ;;
    *)
      [[ -z "$project" ]] || { echo "error: give one Unity project path" >&2; exit 2; }
      project="$1"
      shift
      ;;
  esac
done

[[ -n "$project" ]] || usage 2

if [[ ! -d "$project/Assets" ]]; then
  echo "error: $project does not look like a Unity project (no Assets folder)" >&2
  exit 2
fi

# No target given: build for whatever this machine is.
if [[ ${#targets[@]} -eq 0 ]]; then
  targets=("$(rustc -vV | awk '/^host:/ { print $2 }')")
fi

plugins="$project/Assets/Plugins/Arcane"

# Where a target's artefact lands, and where Unity wants it.
artifact_for() {
  case "$1" in
    *windows*) echo "arcane_sdk.dll" ;;
    *apple*)   echo "libarcane_sdk.dylib" ;;
    *)         echo "libarcane_sdk.so" ;;
  esac
}

folder_for() {
  case "$1" in
    *windows*)        echo "Windows/x86_64" ;;
    *apple*)          echo "macOS" ;;
    aarch64-*linux*)  echo "Linux/aarch64" ;;
    *)                echo "Linux/x86_64" ;;
  esac
}

built=()
for target in "${targets[@]}"; do
  echo "==> cargo build --release --target $target"
  (cd "$repo_root" && cargo build --release --target "$target")

  source_path="$repo_root/target/$target/release/$(artifact_for "$target")"
  [[ -f "$source_path" ]] || { echo "error: $source_path was not produced" >&2; exit 1; }
  built+=("$target|$source_path")
done

# Two Apple slices asked for together become one universal binary, which is what
# a single macOS build has to ship to run on both Intel and Apple Silicon.
apple_slices=()
for entry in "${built[@]}"; do
  [[ "${entry%%|*}" == *apple* ]] && apple_slices+=("${entry#*|}")
done

if [[ ${#apple_slices[@]} -gt 1 ]]; then
  command -v lipo >/dev/null || { echo "error: lipo is needed to merge Apple slices" >&2; exit 1; }
  mkdir -p "$plugins/macOS"
  lipo -create "${apple_slices[@]}" -output "$plugins/macOS/libarcane_sdk.dylib"
  echo "    macOS/libarcane_sdk.dylib (universal)"
fi

for entry in "${built[@]}"; do
  target="${entry%%|*}"
  source_path="${entry#*|}"

  # Already merged above.
  if [[ "$target" == *apple* && ${#apple_slices[@]} -gt 1 ]]; then
    continue
  fi

  destination="$plugins/$(folder_for "$target")"
  mkdir -p "$destination"
  cp "$source_path" "$destination/"
  echo "    $(folder_for "$target")/$(basename "$source_path")"
done

cat <<MESSAGE

Done. Unity will import the plugins on its next focus, and the package's importer
points each one at the platform it was built for.

A native library is loaded once per Editor session: restart the Editor after the
first import, or after replacing a plugin the Editor has already loaded.
MESSAGE
