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
echo "Note: target-cpu=native tunes for THIS machine's exact Apple-Silicon chip (M1/M2/M3/M4)."
echo "For a profile from real usage instead of tests, run before the optimize step:"
echo "    cargo pgo build && $BIN --yolo \"<a representative prompt>\" && cargo pgo optimize build"
