# Running entheai with Osaurus (local & private)

This walks you from zero to `entheai` reading files and running tools **entirely on your Mac**, using [Osaurus](https://github.com/osaurus-ai/osaurus) as the local model server. No cloud, no API key.

> **Prefer the cloud?** Skip all of this: point `entheai.toml` at OpenCode Zen (`base_url = "https://opencode.ai/zen/v1"`, `api_key_env = "OPENCODE_API_KEY"`, `default_model = "zen/deepseek-v4-pro"`) and `export OPENCODE_API_KEY=…`. This guide is the local path.

## Prerequisites

- **macOS 15.5+**, **Apple Silicon** (M1/M2/M3/M4). Osaurus is Apple-Silicon-only.
- [Homebrew](https://brew.sh).
- A Rust toolchain (to build `entheai`): `curl https://sh.rustup.rs -sSf | sh` or `brew install rust`.

## 1. Install Osaurus

```bash
brew install --cask osaurus
```

Homebrew links the `osaurus` CLI (which lives inside the app bundle) onto your `PATH`. Verify:

```bash
osaurus version
```

If `osaurus` isn't found, link it manually:

```bash
ln -sf "/Applications/Osaurus.app/Contents/MacOS/osaurus" "$(brew --prefix)/bin/osaurus"
```

## 2. Start the local server

```bash
osaurus serve --supervise
```

- Serves the OpenAI-compatible API at **`http://127.0.0.1:1337`** (exactly what `entheai.toml` points at).
- `--supervise` keeps it alive if the menu-bar app quits/crashes.
- Loopback requests need **no API key**.

Check it's up:

```bash
osaurus status          # → running (port 1337)
curl -s http://127.0.0.1:1337/v1/models
```

## 3. Download a model (the one manual step)

Osaurus only downloads models through its **GUI Model Manager** — there's no CLI/API pull. Open it:

```bash
osaurus ui
```

Then **Settings (⌘,) → Models → Download**. Recommended first model:

| Model id (for the API) | Size | Why |
|---|---|---|
| **`gemma-4-e2b-it-4bit`** | ~1.5 GB | Small, fast, and **supports tool-calling** — which entheai's agentic loop requires. Best starter. |
| `qwen3.6-27b-mxfp4` | large | Stronger, needs more RAM. |
| `minimax-m3-coder-small` | large | Coder-focused MoE. |
| `foundation` | 0 (macOS 26+) | Apple's built-in model, no download. |

Models load **on first request** (the first call pays a one-time cold-load; later calls are fast).

Confirm the model is ready (the string under `id` is what you put in config):

```bash
osaurus list                                    # downloaded models
curl -s http://127.0.0.1:1337/v1/models | jq .  # ready-to-serve models
```

## 4. Verify the raw API works

```bash
curl http://127.0.0.1:1337/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemma-4-e2b-it-4bit",
    "messages": [{"role":"user","content":"Reply with exactly: pong"}],
    "max_tokens": 20
  }'
```

You should get a completion containing `pong`.

## 5. Point entheai at your model

Edit `entheai.toml` so `default_model` uses the **exact id from `/v1/models`** (prefixed with the provider name `osaurus/`):

```toml
default_model = "osaurus/gemma-4-e2b-it-4bit"

[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"
```

> The part after `osaurus/` must match an id from `curl .../v1/models`. `qwen3-coder` is **not** a real Osaurus id — that's why the default failed.

## 6. Run entheai

```bash
cargo build --release
./target/release/entheai --yolo "read Cargo.toml and list the workspace crates"
```

With `--yolo`, entheai auto-approves tool calls. It'll call `read_file`, get the contents, and answer. Drop `--yolo` to approve each tool call at a `[y/N]` prompt.

## Troubleshooting

- **`could not reach model provider at http://127.0.0.1:1337/...`** — Osaurus isn't running. `osaurus serve --supervise`, then `osaurus status`.
- **`provider returned 404 … model not found`** — your `default_model` id doesn't match a downloaded model. Run `curl -s http://127.0.0.1:1337/v1/models` and copy an exact `id`.
- **First call is slow** — cold model load; subsequent calls are fast. Osaurus keeps a model resident ~15 min idle by default.
- **The model ignores tools / never calls `read_file`** — pick a tool-calling-capable model (`gemma-4-e2b-it-4bit` works). Very small non-tool models won't drive the agentic loop.
- **Stop the server** — `osaurus stop`.

## Automated setup

`scripts/setup-osaurus.sh` does steps 1–4 for you (install, start, verify) and walks you through the GUI model download, then prints the exact `entheai.toml` line to use.
