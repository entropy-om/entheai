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

Create `entheai.toml` in the project root:

```toml
[providers.zen]
base_url = "https://opencode.ai/zen/v1"
api_key_env = "OPENCODE_API_KEY"

default_model = "zen/deepseek-v4-pro"
```

Set your API key:

```bash
export OPENCODE_API_KEY="your-key-here"
```

## Run

```bash
# One-shot prompt
cargo run -- "explain the architecture of this project"

# Interactive TUI (no prompt argument)
cargo run

# YOLO mode (auto-approve all tool calls)
cargo run -- --yolo "fix all clippy warnings"

# Custom config, custom model
cargo run -- --config my-config.toml --model zen/deepseek-v4-flash "refactor the auth module"
```

## First session

In the TUI:
1. Type a prompt, press Enter
2. The agent thinks, may call tools (read files, run shell commands)
3. Permission prompts appear for gated tools — press `y` to allow, `n` to deny
4. Results stream back into the conversation
5. Press `q` (empty input) or `Esc` to quit

## Music

```bash
# In the TUI input, type:
/radio https://www.youtube.com/watch?v=...
/radio pause
/radio next
```

Or use shortcuts: `Ctrl-P` (pause), `Ctrl-N` (next track).

Requires `yt-dlp`:

```bash
brew install yt-dlp
```

## Run tests

```bash
# Fast parallel tests (recommended)
cargo nextest run --workspace --all-targets --all-features

# Full CI gate
./scripts/check.sh
```
