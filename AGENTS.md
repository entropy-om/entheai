# AGENTS.md — entheai

> macOS/Apple-Silicon native hybrid coding agent CLI (Rust workspace).

## Essential commands

```bash
./scripts/check.sh             # full CI gate: fmt (--check) + clippy (-D warnings) + tests
cargo build --release          # optimized build (fat LTO, single codegen unit, target-cpu=native)
cargo nextest run --workspace --all-targets --all-features   # fast parallel tests (preferred)
cargo test --workspace --all-targets --all-features          # fallback test runner
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check     # NB: `cargo fmt` takes `--all`, not `--workspace`
```

Run the binary: `cargo run -- --help` or `./target/release/entheai "your prompt"`.

## Build environment

- **Target**: `aarch64-apple-darwin` only (Apple Silicon). Pinned Rust toolchain `1.96.0` (MSRV `1.80`).
- **Allocator**: `mimalloc` on macOS (wired as `#[global_allocator]` in `bin/entheai/src/main.rs`).
- **Release profile**: `opt-level=3`, `lto="fat"`, `codegen-units=1`, `panic="unwind"` (never abort — sub-agent tokio-task panics must be catchable).
- **`.cargo/config.toml`**: sets `target-cpu=native` and `-Wl,-dead_strip` for macOS; binaries are not portable to older CPUs.
- **`scripts/check.sh`** sets `CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=unpacked` (and same for test) to speed up dev compilation by skipping dSYM bundling.

## Workspace crate map

```
Cargo.toml                          # workspace root (resolver=2)
├── bin/entheai/                    # CLI binary (clap, tokio, sentry, mimalloc)
├── crates/config/                  # entheai-config: TOML → Config deserialization
├── crates/providers/               # entheai-providers: Provider trait + OpenAiCompatProvider
├── crates/core/                    # entheai-core: Agent loop (streaming + tool-dispatch)
├── crates/tools/                   # entheai-tools: Tool trait + ToolRegistry + built-in tools
├── crates/permission/              # entheai-permission: Policy (yolo/allowlist/ask) + Prompter
└── crates/radio/                   # entheai-radio: in-TUI music (yt-dlp download → rodio playback)
```

Crate names use dashes (`entheai-core`) but Rust module names use underscores (`entheai_core`). Every crate is version `0.1.0`.

## Architecture & data flow

1. **`bin/entheai/src/main.rs`** parses CLI args (prompt, --config, --model, --yolo), loads `entheai.toml` via `entheai_config::Config::from_toml_str`, resolves `<provider>/<model>` to an `OpenAiCompatProvider`, registers built-in tools in a `ToolRegistry`, sets up the permission `Policy`, and calls `agent.run_task()`.
2. **`entheai-core::Agent`** owns a provider + model string. Two modes:
   - `run_turn()` — streaming chat: calls `provider.stream_chat()`, pushes tokens to a `TokenSink` trait (stdout in the CLI), collects full text.
   - `run_task()` — agentic loop: repeatedly calls `provider.complete()` with tool schemas, dispatches tool calls, feeds results back into the message history, hard-capped at **25 turns**.
3. **`entheai_providers::OpenAiCompatProvider`** implements the `Provider` trait. Streaming uses `eventsource-stream` to parse SSE; non-streaming `complete()` deserializes JSON including optional `tool_calls`. Both hit `POST /chat/completions`.
4. **`entheai_tools::ToolRegistry`** stores `Box<dyn Tool>` by name. `schemas()` returns all OpenAI function-tool JSON schemas for the provider. **Built-in tools**: `read_file`, `write_file`, `search`, `run_shell`. All are rooted at the canonicalized `current_dir()`.
5. **`entheai_permission`**: `Policy.decide(tool_name)` returns `Allow` (yolo or allowlisted), `Deny`, or `Ask`. `Ask` falls through to `Prompter::confirm()` which reads `y/N` from stdin.

## Key patterns & conventions

### Traits as extension points
All core extension points use `#[async_trait]`:
- `Provider` — add new model backends
- `Tool` — add new built-in tools
- `TokenSink` — route streaming output (CLI → stdout, TUI → terminal widget)
- `Prompter` — swap permission UI (CLI → stdin, TUI → dialog)

### Tool implementation recipe
Each tool is a struct with a `root`/`cwd` field + `new(root)` constructor, implementing:
- `fn name(&self) -> &str`
- `fn schema(&self) -> serde_json::Value` — OpenAI function-tool JSON with `type`, `function.name`, `function.parameters`
- `async fn call(&self, args: serde_json::Value) -> anyhow::Result<String>` — execute, return text

Register in `main.rs` with `registry.register(Box::new(MyTool::new(root.clone())))`.

### Testing patterns
- **Inline tests**: `#[cfg(test)] mod tests` at the bottom of each source file (not in separate `/tests` directories)
- **Async tests**: `#[tokio::test]` — tokio runtime is spun per test
- **HTTP mocking**: `wiremock::MockServer` for provider integration tests
- **Filesystem**: `tempfile::tempdir()` for tool tests of file I/O
- **Provider mocking**: implement the `Provider` trait directly in test modules (see `core::tests` for `MockProvider` and `ScriptedProvider`)

### Naming & style
- `impl Into<String>` for public constructors (ergonomic, no string clone at call site)
- `anyhow::Result<T>` throughout (no custom error types in v0.1)
- `serde_json::Value` for tool args and schemas (dynamic, matches OpenAI wire format)
- One-line doc comments `///` on public items

## Gotchas

- **`cargo fmt` uses `--all`, not `--workspace`**. `cargo fmt --workspace` will error. Build/clippy/test all use `--workspace`.
- **File-tool path sandboxing**: `resolve_in_root()` in `crates/tools/src/fs.rs` blocks both `..` traversal AND symlink escapes. It canonicalizes the deepest existing ancestor and compares against the canonicalized root. macOS temp dirs are under `/var` (a symlink to `/private/var`), so the root *must* be canonicalized before comparison — callers must pass a canonicalized root (the CLI does this).
- **run_shell uses `kill_on_drop(true)`**: if the tokio task is aborted (e.g. on timeout), the child process is reaped, not orphaned.
- **Hard caps exist everywhere**: 25 max tool-dispatch turns (agent loop), 120s shell timeout, 200 max search results, 100KB max shell output. These prevent runaway API costs and memory blowup.
- **Sentry DSN is hardcoded** in `bin/entheai/src/main.rs:33-35`. Override via `SENTRY_DSN` env var. No PII is sent (`send_default_pii: false`).
- **Model ID format is `<provider>/<model>`** (e.g. `osaurus/qwen3-coder`, `zen/deepseek-v4-pro`). The string is split on the first `/`.
- **Config file is `entheai.toml`** by default. The `[providers.<name>]` key is used to look up the provider config. `api_key_env` names an environment variable to read (not the key itself).
- **Only macOS/Apple Silicon**. Hardware-specific tuning (`target-cpu=native`, `mimalloc`, `-Wl,-dead_strip`) means the binary won't run on Intel Macs or other platforms.

## External services

- **Osaurus**: local inference server on `http://127.0.0.1:1337/v1` (OpenAI-compatible)
- **OpenCode Zen**: cloud gateway at `https://opencode.ai/zen/v1` (DeepSeek V4 Pro/Flash, Qwen, etc.)
- **Sentry**: crash/error reporting with hardcoded DSN (opt-out via `SENTRY_DSN` env)

## Future crates (not yet built)

Notable planned additions per the design spec in `docs/superpowers/`: `router`, `agents` (fan-out), `memory`, `learning`, `mcp`, `skills`, `plugins`, `session`, `comms`, `tui`, `viz`, `dogfeed`, `compaction`, `honcho`, `sonar`.
