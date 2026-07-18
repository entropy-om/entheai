#!/usr/bin/env bash
# Dev helper: build + launch the entheai TUI (fast dev profile) from the repo root,
# so the tool sandbox (cwd) and .env resolve correctly. Extra flags pass through:
#   ./scripts/dev_tui.sh                                     # plain interactive TUI
#   ./scripts/dev_tui.sh --yolo                              # auto-approve tool calls
#   ./scripts/dev_tui.sh --model openrouter/deepseek/deepseek-chat
set -euo pipefail
cd "$(dirname "$0")/.."
export RUST_BACKTRACE=1
# entheai auto-loads .env; omitting a prompt arg launches the interactive TUI.
exec cargo run -q -p entheai -- "$@"
