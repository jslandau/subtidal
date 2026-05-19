#!/usr/bin/env bash
# scripts/bundle-mac.sh — build a minimal Subtidal.app bundle from cargo output.
#
# Usage (from repo root):
#   scripts/bundle-mac.sh                # release build
#   scripts/bundle-mac.sh --debug        # debug build
#
# Bundle ID `com.subtidal.app` and ad-hoc codesign identity must stay stable
# across rebuilds to preserve TCC (Screen Recording) grants. See
# docs/design-plans/2026-05-18-macos-port.md §"TCC permissions and the .app
# wrapper".

set -euo pipefail

PROFILE="release"
if [[ "${1:-}" == "--debug" ]]; then
  PROFILE="debug"
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

if [[ "$PROFILE" == "release" ]]; then
  cargo build --release
else
  cargo build
fi

BIN="target/${PROFILE}/subtidal"
APP="target/${PROFILE}/Subtidal.app"

if [[ ! -x "$BIN" ]]; then
  echo "error: binary not found at $BIN" >&2
  exit 1
fi

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/subtidal"
cp resources/macos/Info.plist "$APP/Contents/Info.plist"

plutil -lint "$APP/Contents/Info.plist"

codesign --force --deep --sign - "$APP"

echo "Built $APP"
