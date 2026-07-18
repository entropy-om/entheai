#!/usr/bin/env bash
# Set up Osaurus locally and get entheai talking to it end-to-end.
# Safe to re-run. Osaurus model DOWNLOAD is GUI-only, so the script walks you
# through that one manual step and automates everything else.
set -euo pipefail

GRN=$'\033[32m'; YEL=$'\033[33m'; RED=$'\033[31m'; BLD=$'\033[1m'; RST=$'\033[0m'
info() { printf '%s==>%s %s\n' "$GRN" "$RST" "$*"; }
warn() { printf '%s!!%s  %s\n' "$YEL" "$RST" "$*"; }
die()  { printf '%sxx%s  %s\n' "$RED" "$RST" "$*" >&2; exit 1; }
ask()  { local a; read -r -p "$1 [y/N] " a; [[ "$a" =~ ^[Yy]$ ]]; }

PORT="${OSU_PORT:-1337}"
BASE="http://127.0.0.1:${PORT}"
APP_CLI="/Applications/Osaurus.app/Contents/MacOS/osaurus"

# --- 1. Preconditions -------------------------------------------------------
[[ "$(uname -s)" == "Darwin" ]] || die "Osaurus is macOS-only."
[[ "$(uname -m)" == "arm64"  ]] || die "Osaurus requires Apple Silicon (arm64)."
command -v brew >/dev/null 2>&1 || die "Homebrew required — see https://brew.sh"

# --- 2. Install Osaurus + link the CLI --------------------------------------
if ! command -v osaurus >/dev/null 2>&1; then
  if [[ ! -x "$APP_CLI" ]]; then
    ask "Osaurus not found. Install with 'brew install --cask osaurus'?" \
      || die "Osaurus is required."
    brew install --cask osaurus
  fi
  if [[ -x "$APP_CLI" ]]; then
    info "Linking the osaurus CLI onto PATH..."
    ln -sf "$APP_CLI" "$(brew --prefix)/bin/osaurus"
  fi
fi
command -v osaurus >/dev/null 2>&1 || die "osaurus CLI still not on PATH."
info "osaurus $(osaurus version 2>/dev/null || echo '(installed)')"

# --- 3. Start the server ----------------------------------------------------
if curl -sf "${BASE}/v1/models" >/dev/null 2>&1; then
  info "Server already up at ${BASE}"
else
  info "Starting Osaurus (osaurus serve --supervise)..."
  osaurus serve --supervise >/dev/null 2>&1 || true
  for i in $(seq 1 30); do
    if curl -sf "${BASE}/v1/models" >/dev/null 2>&1; then break; fi
    sleep 1
    [[ "$i" -eq 30 ]] && die "Server didn't come up at ${BASE}. Try 'osaurus serve' manually."
  done
  info "Server is up at ${BASE}"
fi

# --- 4. Ensure a model is downloaded (GUI only) -----------------------------
models_count() { curl -sf "${BASE}/v1/models" 2>/dev/null | grep -o '"id"' | wc -l | tr -d ' '; }
if [[ "$(models_count)" == "0" ]]; then
  warn "No model downloaded yet — Osaurus pulls models only via its GUI."
  printf '   Opening the Osaurus UI: Settings (Cmd+,) -> Models -> Download.\n'
  printf '   Recommended first model: %sgemma-4-e2b-it-4bit%s (small, tool-calling capable).\n' "$BLD" "$RST"
  osaurus ui >/dev/null 2>&1 || open -a Osaurus 2>/dev/null || true
  info "Waiting for a model to finish downloading (Ctrl-C to abort)..."
  while [[ "$(models_count)" == "0" ]]; do sleep 3; done
fi

# --- 5. Report the ready model id + config ----------------------------------
MODEL_ID="$(curl -sf "${BASE}/v1/models" | sed -n 's/.*"id" *: *"\([^"]*\)".*/\1/p' | head -1)"
[[ -n "$MODEL_ID" ]] || die "Could not read a model id from ${BASE}/v1/models."
info "Ready model: ${BLD}${MODEL_ID}${RST}"
printf '\nPut this in your %sentheai.toml%s:\n' "$BLD" "$RST"
printf '  %sdefault_model = "osaurus/%s"%s\n'  "$GRN" "$MODEL_ID" "$RST"
printf '  %s[providers.osaurus]%s\n'           "$GRN" "$RST"
printf '  %sbase_url = "%s/v1"%s\n\n'           "$GRN" "$BASE" "$RST"

# --- 6. Optional build + smoke test -----------------------------------------
if ask "Build entheai (cargo build --release) now?"; then
  cargo build --release
  info "Try it:"
  printf '  ./target/release/entheai --model osaurus/%s --yolo "read Cargo.toml and list the crates"\n' "$MODEL_ID"
fi
info "Done. 'osaurus status' to check · 'osaurus stop' to stop the server."
