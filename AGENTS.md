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

- **Target**: `aarch64-apple-darwin` only (Apple Silicon). Pinned Rust toolchain `1.96.0` (MSRV `1.94`, required by adk-rust's dependency tree).
- **Allocator**: `mimalloc` on macOS (wired as `#[global_allocator]` in `bin/entheai/src/main.rs`).
- **Release profile**: `opt-level=3`, `lto="fat"`, `codegen-units=1`, `panic="unwind"` (never abort — sub-agent tokio-task panics must be catchable).
- **`.cargo/config.toml`**: sets `target-cpu=native` and `-Wl,-dead_strip` for macOS; binaries are not portable to older CPUs.
- **`scripts/check.sh`** sets `CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=unpacked` (and same for test) to speed up dev compilation by skipping dSYM bundling.

## Workspace crate map

```
Cargo.toml                          # workspace root (resolver=2)
├── bin/entheai/                    # CLI binary (clap, tokio, sentry, mimalloc)
├── bin/entheai-worker/             # federation worker (--serve / --dispatch)
├── bin/entheai-launch/             # native .app launcher (Ghostty window)
├── crates/config/                  # entheai-config: TOML → Config deserialization
├── crates/core/                    # entheai-core: EntheaiAgent (adk-rust-backed agent loop)
├── crates/tools/                   # entheai-tools: Tool trait + ToolRegistry + built-in tools
├── crates/permission/              # entheai-permission: Policy (yolo/allowlist/ask) + Prompter
├── crates/router/                  # entheai-router: role→model resolution + EntheaiAgent factory
├── crates/orchestrator/            # entheai-orchestrator: fan-out decomposition + worktree isolation
├── crates/mapper/                  # entheai-mapper: @{path} input sectioning
├── crates/tui/                     # entheai-tui: interactive ratatui chat UI
├── crates/companion/               # entheai-companion: session beacon window (QR + animation)
├── crates/memory/                  # entheai-memory: 5-namespace SQLite + vector store
├── crates/memory-pp/               # entheai-memory-pp: prompt-processing, frozen nodes, BrainJudge
├── crates/viz/                     # entheai-viz: TUI visualization models (brain ring, swarm)
├── crates/radio/                   # entheai-radio: in-TUI music (yt-dlp download → rodio playback)
├── crates/mcp/                     # entheai-mcp: MCP client + server supervisor
├── crates/skills/                  # entheai-skills: SKILL.md discovery + installer
├── crates/launcher/                # entheai-launcher: native-app window spawn
├── crates/obsidian/                # entheai-obsidian: wiki-sync
├── crates/bus/                     # entheai-bus: NATS event bus (federation)
├── crates/federation/              # entheai-federation: remote fleet dispatch
├── crates/sandbox/                 # entheai-sandbox: isolated execution
├── crates/ultragraph/              # entheai-ultragraph: graph data structure (Rust port)
└── crates/kompress-core/           # kompress-core: context-pruning pipeline (vendored from kompress-ultra)
```

Crate names use dashes (`entheai-core`) but Rust module names use underscores (`entheai_core`). Workspace version: see `Cargo.toml`'s `[workspace.package]`.

## Architecture & data flow

`crates/core` is built on [adk-rust](https://github.com/zavora-ai/adk-rust) (pinned `1.0.0`) — there is no hand-rolled `Provider`/streaming client anymore; model calls go through adk-rust's own `Llm` implementations (`adk_rust::model::openai::OpenAIClient` for OpenAI-compatible endpoints).

1. **`bin/entheai/src/main.rs`** parses CLI args (prompt, `--config`, `--model`, `--yolo`, `--fanout`), loads `entheai.toml` via `entheai_config::Config::from_toml_str`, registers built-in tools in a `ToolRegistry`, sets up the permission `Policy`, and builds an `EntheaiAgent` (via `EntheaiAgent::build_auto`, which picks the memory-aware or instruction-only constructor).
2. **`entheai_core::EntheaiAgent`** (`crates/core/src/entheai_agent.rs`) wraps an `adk_rust::agent::LlmAgentBuilder` + `Runner` + `SessionService`. Two entry points:
   - `run_to_text()` / `run()` — one fresh message, one fresh session.
   - `run_with_history()` — seeds prior `(role, text)` turns into the session via `SessionService::append_event` before running the new message (what the interactive TUI uses, since it carries full conversation history forward).
   `crates/core/src/event_bridge.rs`'s `run_with_events()` drives the resulting `adk_rust::EventStream` and translates it into the TUI-facing `AgentEvent` enum (`Thinking`/`Token`/`ToolStarted`/`ToolFinished`/`FrozenWoke`), and — when memory is enabled — records the final answer's trajectory and raw transcript once the run completes.
3. **`entheai_core::model_resolve::resolve_model`** parses `"<provider>/<model>"` into an `Arc<dyn adk_rust::Llm>` using `[providers.<name>]` config (`base_url` + optional `api_key_env`).
4. **`entheai_tools::ToolRegistry`** stores `Arc<dyn Tool>` by name (non-consuming `to_tools()` lets one registry back multiple `EntheaiAgent` builds — needed since the interactive TUI builds a fresh agent every turn). `crates/core/src/adk_tool_adapter.rs`'s `AdkToolAdapter` wraps each `entheai_tools::Tool` (+ `Policy` + `Prompter`) as an `adk_rust::Tool`. **Built-in tools**: `read_file`, `write_file`, `search`, `run_shell`. All are rooted at the canonicalized `current_dir()`.
5. **`entheai_permission`**: `Policy.decide(tool_name)` returns `Allow` (yolo or allowlisted), `Deny`, or `Ask`. `Ask` falls through to `Prompter::confirm()` which reads `y/N` from stdin (CLI) or forwards to the TUI's permission modal.
6. **`entheai-companion`** (separate binary, `crates/companion/src/main.rs`): spawned as a child process by the main binary whenever `[companion].enabled = true` (and `--no-companion` is not passed). A 180×180 px borderless always-on-top floating window (winit + softbuffer). Shows an animated breathing glow with a QR code encoding `{sid, host, port, cwd}`. Drives a four-state animation:
   - **idle** — slow teal pulse (3s cycle) when TUI is waiting for input
   - **working** — fast teal pulse (1.5s) + orbiting spinner while the agent runs
   - **permission_pending** — magenta pulse (1s) + "?" glyph when a tool is gated
   - **error** — red dim pulse (4s) on errors
   State changes arrive over a Unix socket (`$TMPDIR/entheai-<sid>.sock`) as `StateChange` JSON lines. Clicking copies `http://<host>.local:9876/session/<sid>` to clipboard. On socket close, fades out over 500ms and exits. Full spec: `docs/superpowers/specs/2026-07-18-entheai-companion-design.md`.

### Companion config (`entheai.toml`)

```toml
[companion]
enabled = true          # spawn companion window (default: true)
always_on_top = true    # float above other windows (default: true)
```

CLI: `--no-companion` disables for the session.

## Key patterns & conventions

### Traits as extension points
All core extension points use `#[async_trait]`:
- `adk_rust::Llm` — add new model backends (adk-rust ships OpenAI/Anthropic/Gemini/etc.; `entheai_core::model_resolve` only wires up OpenAI-compatible)
- `Tool` (`entheai_tools`) — add new built-in tools, wrapped as `adk_rust::Tool` via `AdkToolAdapter`
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
- **HTTP mocking**: `wiremock::MockServer` mocking the OpenAI-compatible `/chat/completions` SSE endpoint (see `crates/core/src/entheai_agent.rs` and `crates/core/tests/parity.rs` for the request/response fixture shapes)
- **Filesystem**: `tempfile::tempdir()` for tool tests of file I/O
- **Fake `Llm`/`Tool` impls**: implement `adk_rust::Llm` or `adk_rust::Tool` directly in test modules when a scenario needs more control than an SSE mock gives (see `crates/memory-pp/src/judge.rs`'s `FakeLlm`)

### Naming & style
- `impl Into<String>` for public constructors (ergonomic, no string clone at call site)
- `anyhow::Result<T>` throughout (no custom error types)
- `serde_json::Value` for tool args and schemas (dynamic, matches OpenAI wire format)
- One-line doc comments `///` on public items

## Gotchas

- **`cargo fmt` uses `--all`, not `--workspace`**. `cargo fmt --workspace` will error. Build/clippy/test all use `--workspace`.
- **File-tool path sandboxing**: `resolve_in_root()` in `crates/tools/src/fs.rs` blocks both `..` traversal AND symlink escapes. It canonicalizes the deepest existing ancestor and compares against the canonicalized root. macOS temp dirs are under `/var` (a symlink to `/private/var`), so the root *must* be canonicalized before comparison — callers must pass a canonicalized root (the CLI does this).
- **run_shell uses `kill_on_drop(true)`**: if the tokio task is aborted (e.g. on timeout), the child process is reaped, not orphaned.
- **Hard caps exist everywhere**: `[router].max_turns` (default 200, `u32::MAX` under `--yolo`) tool-dispatch turns per `EntheaiAgent`, 120s shell timeout, 200 max search results, 100KB max shell output. These prevent runaway API costs and memory blowup.
- **`[inference].request_timeout_secs`/`.retries` are inert.** adk-rust 1.0.0's `OpenAIClient` hardcodes `reqwest::Client::new()` with no timeout/retry builder surface — a confirmed gap, not a bug in entheai's wiring. `temperature`/`max_tokens` still work (`LlmAgentBuilder::temperature`/`max_output_tokens`).
- **Sentry DSN is hardcoded** in `bin/entheai/src/main.rs`. Override via `SENTRY_DSN` env var. No PII is sent (`send_default_pii: false`).
- **Model ID format is `<provider>/<model>`** (e.g. `osaurus/qwen3-coder`, `zen/deepseek-v4-pro`). The string is split on the first `/` in `entheai_core::model_resolve::resolve_model`.
- **Config file is `entheai.toml`** by default. The `[providers.<name>]` key is used to look up the provider config. `api_key_env` names an environment variable to read (not the key itself).
- **Only macOS/Apple Silicon**. Hardware-specific tuning (`target-cpu=native`, `mimalloc`, `-Wl,-dead_strip`) means the binary won't run on Intel Macs or other platforms.

## External services

- **Osaurus**: local inference server on `http://127.0.0.1:1337/v1` (OpenAI-compatible)
- **OpenCode Zen**: cloud gateway at `https://opencode.ai/zen/v1` (DeepSeek V4 Pro/Flash, Qwen, etc.)
- **Sentry**: crash/error reporting with hardcoded DSN (opt-out via `SENTRY_DSN` env)
