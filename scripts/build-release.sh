#!/usr/bin/env bash
# Production build — PGO-optimized, state-of-the-art release of entheai for Apple Silicon.
#
# Layers Profile-Guided Optimization on top of the already-aggressive release profile
# (lto="fat", codegen-units=1) and .cargo/config.toml (target-cpu=native, -dead_strip).
# cargo-pgo COMBINES with those macOS `[target.*] rustflags`, so native+LTO are preserved.
#
# Profiles are gathered by running the TEST SUITE (deterministic, needs no API keys) — it
# exercises the hot paths (agent loop with mock providers, SSE parsing, tool execution).
# For an even better profile, gather from a real workload instead (see the note at the end).
#
# BOLT is intentionally NOT used: it only handles ELF binaries (Linux), not macOS Mach-O.
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="aarch64-apple-darwin"
BIN="target/${TARGET}/release/entheai"

command -v cargo >/dev/null 2>&1 || { echo "cargo is required"; exit 1; }

# cargo-pgo needs the llvm-profdata tool from the llvm-tools rustup component.
rustup component add llvm-tools-preview >/dev/null 2>&1 || true
if ! cargo pgo --version >/dev/null 2>&1; then
  echo "==> installing cargo-pgo…"
  cargo install cargo-pgo
fi

echo "==> environment check (cargo pgo info)"
cargo pgo info || true

echo "==> [1/2] instrument + gather profiles from the test suite…"
cargo pgo instrument test

echo "==> [2/2] build the PGO-optimized release…"
cargo pgo optimize build

echo
if [ -f "$BIN" ]; then
  echo "✅ Optimized binary: $BIN"
  ls -lh "$BIN"
else
  echo "⚠️  Expected the optimized binary at $BIN — check the cargo-pgo output above."
  exit 1
fi
echo

mkdir -p dist

echo "==> assembling entheai.app"
APP="dist/entheai.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/shaders"
cp "target/${TARGET}/release/entheai-launch"     "$APP/Contents/MacOS/entheai-launch"
cp "target/${TARGET}/release/entheai"            "$APP/Contents/MacOS/entheai"
cp "target/${TARGET}/release/entheai-companion"  "$APP/Contents/MacOS/entheai-companion"
cp crates/launcher/assets/ghostty-minimal.conf.tmpl "$APP/Contents/Resources/ghostty-minimal.conf.tmpl"
cp crates/launcher/assets/rain_on_glass.glsl        "$APP/Contents/Resources/shaders/rain_on_glass.glsl"
cp bin/entheai/resources/Info.plist "$APP/Contents/Info.plist"
if [ -f docs/images/hero.jpg ]; then
  ICONSET="$(mktemp -d)/AppIcon.iconset"; mkdir -p "$ICONSET"
  for s in 16 32 64 128 256 512; do
    sips -z "$s" "$s" docs/images/hero.jpg --out "$ICONSET/icon_${s}x${s}.png" >/dev/null 2>&1 || true
    sips -z $((s*2)) $((s*2)) docs/images/hero.jpg --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null 2>&1 || true
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns" 2>/dev/null || echo "   (icon generation skipped)"
fi
echo "==> ad-hoc codesigning entheai.app"
codesign --force --deep --sign - "$APP"
echo "==> zipping entheai-app-macos-arm64.zip"
( cd dist && ditto -c -k --keepParent entheai.app entheai-app-macos-arm64.zip )
echo "    built: dist/entheai-app-macos-arm64.zip"
echo
echo "Note: target-cpu=native tunes for THIS machine's exact Apple-Silicon chip (M1/M2/M3/M4)."
echo "For a profile from real usage instead of tests, run before the optimize step:"
echo "    cargo pgo build && $BIN --yolo \"<a representative prompt>\" && cargo pgo optimize build"
