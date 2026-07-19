# Changelog

All notable changes to `entheai` are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/); versioning: strict
[SemVer](https://semver.org/) — see [VERSIONING.md](VERSIONING.md).

## [Unreleased]

## [0.1.0] - 2026-07-19

First versioned baseline — the v0.1 thin-but-complete slice.

### Added
- **Router** — config-driven role→model resolution (`[router]` / `[agents.*]`) + a reusable agent factory across all providers.
- **Fan-out orchestration** — orchestrator decomposes a task; parallel sub-agents run model-matched. Coders execute in isolated **git worktrees** → optional verify → integrate onto a branch with conflict detection; read-only analysis fallback outside a git repo; live progress in the TUI.
- **MCP client + supervisor** — spawn any configured Model Context Protocol server at startup; its tools are exposed to the agent as `<server>__<tool>`.
- **Skills** — discover `SKILL.md` skills (Claude Agent-Skills format), advertise them via a system prompt, and load one on demand with the `skill` tool.
- **Token streaming** — SSE `stream_complete` with tool-call assembly; answers stream live into the TUI.
- **Tools** — root-scoped, symlink-guarded `read_file` / `write_file` / `edit_file` (surgical unique string-replace) / `run_shell` (timeout + kill) / `search`.
- **Memory engine** — 5-namespace SQLite + vector store, wired into the agent loop (pre-task retrieval, tool-output spillover, trajectory/learning recording).
- **Companion beacon** — always-on-top window rendering a QR for phone/tablet session pairing over the tailnet.
- **TUI** — ratatui chat with streaming output, inline tool progress, permission modal, and an in-TUI radio player.
- **Providers** — OpenAI-compatible streaming/non-streaming for DeepSeek, OpenRouter, Hugging Face, OpenCode Zen, and local Osaurus.
- **Ops** — perf-first release profile (mimalloc, fat LTO, PGO build script), Sentry crash reporting (PII disabled), typed errors (`thiserror` in libs, `anyhow` in the binary).

[Unreleased]: https://github.com/peterlodri-sec/entheai/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/peterlodri-sec/entheai/releases/tag/v0.1.0
