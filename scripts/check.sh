#!/usr/bin/env bash
set -euo pipefail

# macOS: skip dSYM bundling for dev/test builds (leave debug info in object files) —
# drastically cuts clippy/test compile time on large workspaces. LLDB still resolves symbols.
export CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=unpacked
export CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=unpacked

echo "=> Checking formatting..."
# NB: `cargo fmt` takes `--all` (not `--workspace`); the latter is valid for build/clippy/test only.
cargo fmt --all -- --check

echo "=> Running clippy..."
cargo clippy --workspace --all-targets --all-features -- -D warnings

echo "=> Running tests..."
# cargo-nextest runs each test in an isolated process and schedules well across
# Apple Silicon P/E cores; fall back to `cargo test` if it isn't installed.
if command -v cargo-nextest >/dev/null 2>&1; then
    cargo nextest run --workspace --all-targets --all-features
else
    echo "  (tip: install 'cargo-nextest' for faster, isolated test runs)"
    cargo test --workspace --all-targets --all-features
fi
