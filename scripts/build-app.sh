#!/bin/zsh
# Builds Canopy.app into ./build.
#
#   scripts/build-app.sh            release build, signed with your first
#                                   code-signing identity (or ad hoc)
#   CODESIGN_IDENTITY=- scripts/build-app.sh   force ad-hoc signing
#
# macOS ties the Accessibility permission to the app's signature. With a real
# identity the permission survives rebuilds; with ad-hoc signing you have to
# allow Canopy again after every rebuild.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release -p canopy -p canopy-test-windows
BIN=target/release/Canopy

APP=build/Canopy.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Canopy"
cp Resources/Info.plist "$APP/Contents/Info.plist"

IDENTITY="${CODESIGN_IDENTITY:-}"
if [[ -z "$IDENTITY" ]]; then
  IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null | awk 'NR==1 && $2 ~ /^[0-9A-F]{40}$/ {print $2}')"
  [[ -z "$IDENTITY" ]] && IDENTITY="-"
fi
if codesign --force --options runtime --entitlements Resources/Canopy.entitlements --sign "$IDENTITY" "$APP" >/dev/null 2>&1; then
  echo "Built $APP (signed: ${IDENTITY:0:8}…)"
else
  # The signing key is out of reach: usually a locked screen or keychain.
  codesign --force --sign - "$APP"
  echo "Built $APP, but SIGNED AD HOC: you would have to allow Accessibility again. Unlock the Mac and rerun this." >&2
fi
