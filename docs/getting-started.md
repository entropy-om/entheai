# Getting Started

## Prerequisites

- macOS 15.5+ on Apple Silicon (M1/M2/M3/M4)
- Rust 1.96.0 (see `rust-toolchain.toml`)
- A graphics-capable terminal: [Ghostty](https://ghostty.org), [Kitty](https://sw.kovidgoyal.net/kitty/), or [WezTerm](https://wezfurlong.org/wezterm/)
- Optional: [Tailscale](https://tailscale.com) (for federation), [Osaurus](https://github.com/peterlodri-sec/Osaurus) (for local inference)

## Install

```bash
git clone https://github.com/entropy-om/entheai.git
cd entheai
```

## Build

```bash
# Development build (fast compile, slower runtime)
cargo build

# Optimized release build (slow compile, fast runtime)
cargo build --release
```

The release binary lands at `target/release/entheai`.

## Configure

Create `entheai.toml` in the project root (or rely on the built-in defaults, which set exactly this — `deepseek`, `gemini`, `openrouter` and the keyless `vaked` providers are built in):

```toml
default_model = "deepseek/deepseek-v4-flash"   # interactive: V4 Flash

[router]
orchestrator = "deepseek/deepseek-v4-pro"       # fan-out planning, coder + reviewer: V4 Pro
```

Note: `default_model` alone also becomes the fan-out model for every role — set `[router].orchestrator` (as above) to keep planning, coder and reviewer on V4 Pro while chatting on Flash.

Set your API key:

```bash
export DEEPSEEK_API_KEY="your-key-here"
```

No key at all? Pass `--model vaked/qwen3-coder:30b` to run on the keyless free tier (`--fanout` degrades to it automatically). Config lookup order: `./entheai.toml` (or `--config <path>`) -> `~/.config/entheai/entheai.toml` -> `~/.config/entheai/config.toml` -> built-in defaults. Full key reference: [configuration.md](configuration.md).

## Run

```bash
# One-shot prompt
cargo run -- "explain the architecture of this project"

# Interactive TUI (no prompt argument)
cargo run

# YOLO mode (auto-approve all tool calls)
cargo run -- --yolo "fix all clippy warnings"

# Custom config, custom model
cargo run -- --config my-config.toml --model deepseek/deepseek-v4-pro "refactor the auth module"
```

## First session

In the TUI:
1. Type a prompt, press Enter
2. The agent thinks, may call tools (read files, run shell commands)
3. Permission prompts appear for gated tools — press `y` to allow, `n` to deny
4. Results stream back into the conversation
5. Press `q` (empty input) or `Esc` to quit

## Music

An ambient loop plays automatically — one bundled track, embedded in the
binary, no setup required.

```bash
# In the TUI input, type:
/radio pause
/radio next   # restart the track from the beginning
/radio stop
```

Or use shortcuts: `Ctrl-P` (pause), `Ctrl-N` (restart).

## Run tests

```bash
# Fast parallel tests (recommended)
cargo nextest run --workspace --all-targets --all-features

# Full CI gate
./scripts/check.sh
```
