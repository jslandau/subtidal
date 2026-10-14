#!/usr/bin/env bash
# scripts/check-macos-version.sh — fail when the bundled app's version
# drifted from Cargo.toml (F15).
#
# Usage: scripts/check-macos-version.sh [path/to/Subtidal.app]
# Defaults to target/release/Subtidal.app. Exits 1 on mismatch or when the
# bundle's Info.plist still contains an unsubstituted placeholder.

set -euo pipefail

APP="${1:-target/release/Subtidal.app}"
PLIST="$APP/Contents/Info.plist"

if [[ ! -f "$PLIST" ]]; then
  echo "error: $PLIST not found (run scripts/bundle-mac.sh first)" >&2
  exit 1
fi

CARGO_VERSION="$(sed -n '/^\[package\]/,/^\[/ s/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)"
if [[ -z "$CARGO_VERSION" ]]; then
  echo "error: could not parse version from Cargo.toml" >&2
  exit 1
fi

if grep -q '@CFBUNDLE_VERSION@' "$PLIST"; then
  echo "error: $PLIST contains an unsubstituted @CFBUNDLE_VERSION@ placeholder" >&2
  exit 1
fi

PLIST_VERSION="$("/usr/libexec/PlistBuddy" -c 'Print :CFBundleShortVersionString' "$PLIST")"

if [[ "$PLIST_VERSION" != "$CARGO_VERSION" ]]; then
  echo "error: version drift — Info.plist has '$PLIST_VERSION', Cargo.toml has '$CARGO_VERSION'" >&2
  exit 1
fi

echo "ok: bundled version $PLIST_VERSION matches Cargo.toml"
