# Changelog

All notable changes to `entheai` are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/); versioning: strict
[SemVer](https://semver.org/) — see [VERSIONING.md](VERSIONING.md).

## [Unreleased]

### Added
- `entheai --skills list` — list installed skills (name, description, path), the companion to `--skills add`.
- `entheai --skills remove <name>` — remove an installed skill by name (slugified → traversal-safe, scoped to the skills dir). Completes the add/list/remove surface.
- **Federation F2.1 — distributed swarm (opt-in `[federation]`).** New `entheai-federation` crate (JetStream work-queue + object-store git-bundles) + `entheai-worker --serve`/`--dispatch`: a coder task can run on another tailnet node — the dispatcher bundles the repo, enqueues a `WorkItem`, and applies the worker's delta to a `fed/…` branch; the worker pulls, materializes, runs the coder, and bundles the result back. Live-verified end-to-end.
- **Federation F2.2 — fan-out offload.** `entheai --fanout` now runs its coder sub-tasks on the fleet when `[federation]` is enabled and a worker is serving: a `CoderExecutor` seam in `run_fanout` (orchestrator stays NATS-agnostic — trait only), a worker **presence heartbeat** (`count_workers` gates dispatch), and a `FederationExecutor` that dispatches each coder and **squash-applies** the delta into its worktree so the existing commit/verify/integrate path is unchanged. Per-coder **local fallback** on no-worker/timeout/no-change; federation off → byte-identical to before. Executor path live-verified (presence + dispatch + squash-apply); full decompose→integrate offload wired (worker securefs hardening is F2.3).

## [0.2.0] - 2026-07-20

The v0.2 slice — federation, richer surfaces, and a portable build. All additive:
the default `cargo build` keeps the full macOS experience.

### Added
- **NATS federation — event bus (F1)** — a new `entheai-bus` crate and opt-in `[nats]` config publish every `--fanout` run's lifecycle to `entheai.fanout.<session>.{decomposed,coder.started,coder.finished,integrating,done}` on a NATS hub, so any tailnet subscriber can watch runs live. Fully fail-safe (disabled/unreachable → local run); the orchestrator stays NATS-agnostic.
- **`entheai --skills add <url>`** — install a skill from the web via layered discovery: `/.well-known/skills.json` (native manifest) → `/llms.txt` (works against Stripe et al.) → the page. Writes `skills/<slug>/SKILL.md`. Path-traversal-safe slug, SSRF-guarded sub-fetch, bounded fetch (15s/1 MiB), skip-if-exists, provenance stamped.
- **Obsidian wiki-sync** — per-session, fail-safe sync of the repo into an Obsidian vault (`[obsidian]`): docs mirror with wikilink/asset rewriting, an architecture generator, session/section indexes + Home MOC, a debounced watcher, and a best-effort MCP nudge.
- **Native app** — `entheai --app` opens a minimalist Ghostty window (`entheai-launch` + the `entheai-launcher` crate, bundled shader/config); `entheai --doctor` installs the rain-on-glass shader into your own `~/.config/ghostty/config`.
- **Live swarm visualization** — an inline ratatui swarm graph during fan-out (`entheai-viz`, `[viz]`, on by default), with a `Ctrl-V` / `/viz` full view and `/workers list|stop|debug` against the in-flight `WorkerPool`.
- **Memory inspection CLI** — `entheai --memory list|search|stats`.
- **Mapper** — `entheai-mapper` routes task text (with `@{path}` extraction + resolved file context) through before decompose.
- **Config surface** — extensive knobs across `[router]`, `[inference]`, `[tools]`, `[permission]`, `[memory]`, `[viz]`, `[companion]`, `[radio]`, `[telemetry]`, `[mcp_defaults]`.

### Changed
- **Portable headless build** — GUI (companion/winit/wayland/drm) and audio (radio/alsa) moved behind default features; `cargo build --no-default-features` now builds the binary with **zero system libraries**. Sentry switched to the **rustls** transport (drops `openssl-sys`/`native-tls`). The default build is unchanged.
- **TUI-safe logging** — a log backend that always writes to a file and mirrors to stderr only outside the alternate-screen TUI.

### Fixed
- **Fan-out coder decomposition** — the git-worktree (v2) path reused the *read-only* decompose prompt, so edit tasks decomposed to explore-only and integrated nothing. Now uses a coder-oriented prompt and guarantees at least one coder sub-task.
- **Bounded external inputs** — MCP initialize/request timeouts, a streaming-capped shell reader (kills the child at the cap), capped file reads, and MCP reader line-length caps.

### Performance
- Provider trait borrows messages/tools instead of cloning per turn (drops per-turn O(n²) history + schema clones).
- TUI per-message line cache (O(n²) → O(delta)/token) rendering from a borrowed slice.
- Obsidian scan reuses an mtime+len read cache — a debounced tick re-reads only changed files.

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

[Unreleased]: https://github.com/peterlodri-sec/entheai/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/peterlodri-sec/entheai/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/peterlodri-sec/entheai/releases/tag/v0.1.0
