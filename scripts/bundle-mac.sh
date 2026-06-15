#!/usr/bin/env bash
# scripts/bundle-mac.sh — build a minimal Subtidal.app bundle from cargo output.
#
# Usage (from repo root):
#   scripts/bundle-mac.sh                # release build
#   scripts/bundle-mac.sh --debug        # debug build
#
# Bundle ID `com.subtidal.app` and ad-hoc codesign identity should stay stable
# across rebuilds to preserve TCC grants. Audio Capture permission persists even
# under ad-hoc signing; stable signing becomes relevant if future features
# require other TCC services.

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
cp resources/macos/tray-icon-template.png "$APP/Contents/Resources/"

# Generate a Dock/Finder app icon from the canonical SVG asset.
APP_ICON_SVG="assets/icons/hicolor/scalable/apps/subtidal.svg"
ICONSET_DIR="target/${PROFILE}/Subtidal.iconset"
APP_ICON_ICNS="$APP/Contents/Resources/Subtidal.icns"
rm -rf "$ICONSET_DIR"
mkdir -p "$ICONSET_DIR"
for size in 16 32 128 256 512; do
  sips -s format png -z "$size" "$size" "$APP_ICON_SVG" --out "$ICONSET_DIR/icon_${size}x${size}.png" >/dev/null
  retina_size=$((size * 2))
  sips -s format png -z "$retina_size" "$retina_size" "$APP_ICON_SVG" --out "$ICONSET_DIR/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET_DIR" -o "$APP_ICON_ICNS"
rm -rf "$ICONSET_DIR"

# Bundle the ort WebGPU runtime dylib next to the binary and wire @rpath so the
# binary can resolve `@rpath/libwebgpu_dawn.dylib` without DYLD_LIBRARY_PATH.
# Required for the bundle to launch via LaunchServices (`open Subtidal.app`),
# which is itself required for TCC (Audio Capture) prompts to surface.
DYLIB_SRC="target/${PROFILE}/libwebgpu_dawn.dylib"
if [[ -f "$DYLIB_SRC" ]]; then
  cp "$DYLIB_SRC" "$APP/Contents/MacOS/libwebgpu_dawn.dylib"
  # Drop any pre-existing @executable_path rpath before re-adding (idempotent reruns).
  install_name_tool -delete_rpath @executable_path "$APP/Contents/MacOS/subtidal" 2>/dev/null || true
  install_name_tool -add_rpath @executable_path "$APP/Contents/MacOS/subtidal"
else
  echo "warn: $DYLIB_SRC not found — bundle will fail to launch without DYLD_LIBRARY_PATH" >&2
fi

plutil -lint "$APP/Contents/Info.plist"

codesign --force --deep --sign - "$APP"

echo "Built $APP"
