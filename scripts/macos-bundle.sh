#!/usr/bin/env bash
# Build a signed `speak.app` bundle so the binary is a TCC subject for the
# native Core Audio system-output tap (ADR-0015 / kTCCServiceAudioCapture).
#
# A bare CLI cannot obtain the macOS audio-capture grant — TCC attributes the
# request to the parent terminal and silently mutes the tap. Wrapping the binary
# in a signed .app (with NSAudioCaptureUsageDescription + the audio-input
# entitlement) makes `speak` its own TCC subject so the audio-capture prompt
# fires and the grant persists.
#
# Usage: scripts/macos-bundle.sh [BINARY] [APP_DIR]
#   BINARY  : binary to wrap (default target/debug/speak)
#   APP_DIR : output bundle  (default target/speak.app)
# Env:
#   CODESIGN_IDENTITY : signing identity (default: first codesigning identity)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/debug/speak}"
APP="${2:-$ROOT/target/speak.app}"
IDENTITY="${CODESIGN_IDENTITY:-$(security find-identity -v -p codesigning | awk 'NR==1{print $2}')}"

[ -x "$BIN" ] || { echo "binary not found: $BIN (run 'make build' first)" >&2; exit 1; }
[ -n "$IDENTITY" ] || { echo "no codesigning identity found" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN" "$APP/Contents/MacOS/speak"
cp "$ROOT/Info.plist" "$APP/Contents/Info.plist"

codesign --force \
  --entitlements "$ROOT/packaging/macos/speak.entitlements" \
  -s "$IDENTITY" "$APP"

echo "=== signature ==="
codesign -dv --entitlements - "$APP" 2>&1 | sed -n '1,30p'
echo
echo "built: $APP"
echo "run (in YOUR interactive Terminal, first run prompts for audio capture):"
echo "  $APP/Contents/MacOS/speak record -s output -o /tmp/sys.wav -d 5"
