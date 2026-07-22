# Changelog

All notable changes to `entheai` are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/); versioning: strict
[SemVer](https://semver.org/) — see [VERSIONING.md](VERSIONING.md).

## [Unreleased]

### Added
- **Permission posture + mode (`Shift+Tab` in TUI, `[permission] mode` in config).** Added a runtime permission posture `[plan · auto · yolo · ask]` cycled with `Shift+Tab` in the TUI, tool risk tiers (`Read < Write < Exec < Network < Spawn`), per-tool pins (`always_allow`, `always_ask`, `never`), and subagent tier-ceiling policy propagation (`ENTHEAI_MODE`).
- **Prompt-processing retrieval — Slice 1 (opt-in, `[memory] mode = "prompt-processing"`).** A new raw experiential tier (`crates/memory-pp`): full session transcripts and all tool outputs are captured RAW, content-addressed (idempotent), and retention-pruned. Retrieval runs recall → mesh re-rank → deterministic compression, but the mesh (`ultra-graph`) and compressor (`marqant`) are in-process stubs behind a strict overall deadline in Slice 1 — so retrieval always falls back cleanly to today's top-K, byte-identical, whenever PP is off, empty, erroring, or slow. Default `topk` behaviour is unchanged. Zero changes to `crates/memory`.
- **Prompt-processing — Slice 2 (the real subprocess seams).** The mesh and compressor stubs are replaced by real, process-isolated backends dropped into the *same* trait seams (no upstream change): a stdio-JSON-RPC **mesh sidecar** (`sidecars/ultragraph/serve.py`, `[memory.prompt_processing] sidecar_cmd`) that ranks candidate raw spans — via the user's `ultragraph` 1-bit mesh when importable, else a deterministic lexical reference scorer — and returns **ids only** (the Rust side rehydrates the never-rewritten raw), and a **`mq compress --semantic`** compressor (`marqant_cmd`) with file-arg I/O. Both are bounded by the pipeline's overall deadline, `kill_on_drop`, and capped readers; any spawn/protocol/timeout/missing-tool failure degrades to top-K, so PP is safe even without `python3`/`mq` installed. Set either command to `""` to force the in-process stub.
- **`entheai-ultragraph` — native Rust port of ultra-graph's 1-bit (ternary) inference core.** A new pure-`std` crate porting the deployed-inference path of the user's `ultra-graph` package (BitNet-b1.58 style): the ternary/int8 quantizers, base-3 5-per-byte ternary packing, the byte tokenizer, and the `.ugm` v1 binary loader + topological forward interpreter (dense trees, plain/residual ultra-edges). Verified byte-exact against the Python reference via a committed conformance fixture. Produced through the **recursive-development path** (implemented by `agy` on the user's Ultra models, then independently verified). Inference-only — training/autograd stay in Python.
- **Prompt-processing — native in-process mesh (default).** A `NativeMesh` re-ranks candidate raw spans **in-process with no subprocess** (`[memory.prompt_processing] mesh_backend = "native"`, now the default), dropping the Python sidecar from the default path while keeping it available (`mesh_backend = "sidecar"`). It scores each candidate with an optional `.ugm` reranker (`native_model`, run via `entheai-ultragraph`) over a `FEATURE_DIM=768` feature vector `[query hist | text hist | query⊙text interaction]` — the interaction block is what lets a linear ternary model rank a query×text match — or a deterministic lexical fallback. PP now completes end-to-end — recall → native re-rank → rehydrate raw → compress — with zero external tools.
- **Prompt-processing — a trained ternary `.ugm` reranker (`native_model`).** Ships a real BitNet-b1.58 reranker (`crates/memory-pp/models/reranker.ugm`) + its reproducible training pipeline (`tools/train_reranker.py`): a single dense ternary linear over the 768-d feature, trained with a margin ranking loss + straight-through estimator on synthetic topical triples, exported to `.ugm`. **94.7%** held-out ranking accuracy; the native mesh loads and runs it in-process (Rust acceptance test). Closes the ultra-graph → PP loop; produced through the recursive-development path (trained by `agy`, verified here). Optional (default stays lexical) — a v0 reference model; retrain with real relevance data via `train_reranker.py`.
- **Brain panel (TUI).** An always-on compact side panel beside the chat: a slowly rotating braille pseudo-3D node graph of the agent's faculties (model · tools · context) and the remote fleet, with a live `wk N · nats ●/○ · ctx %` footer. Faculties flare on token generation / tool calls and decay; the fleet ring + NATS indicator come from a throttled 1.5 s presence poll. Toggle with `/brain`, config `[viz] brain` / `brain_width`; auto-hides on narrow terminals. (Slice B — a kitty-graphics true-3D upgrade behind `graphics_capable()` — is a planned follow-on.)
- **Osaurus local-model status (TUI).** The status bar shows whether the local Osaurus endpoint (the `osaurus` provider's `base_url`, default `http://127.0.0.1:1337/v1`) is up and how many models it serves — green `osaurus ● 3` when reachable, dim `osaurus ○` when not. A throttled 5 s background probe (`GET /models`, 600 ms timeout, fail-safe) updates it off the render path, so a slow/absent endpoint never stalls a frame. Built via the recursive-development path (agy).
- **Automatic Pomodoro timer (TUI).** The status bar now carries an always-on, pure-ASCII 25-min-work / 5-min-break Pomodoro (`WORK 24:59` green, `BREAK 04:12` cyan) that cycles from launch with no command needed. It's a pure wall-clock model in `crates/viz` (`Pomodoro::at(elapsed)`), so it tracks real minutes; an idle session repaints it at ~1 Hz only when the countdown digit changes (no per-frame idle cost).
- **Federation F2.3 — worker confinement + fleet visibility.** Each coder now runs in a self-sandboxing `entheai-worker --sandbox-run` child (new `entheai-sandbox` crate), governed by `[federation] sandbox = "strict" | "permissive" | "off"` (default `permissive`): **Linux** applies a Landlock filesystem jail + seccomp syscall denylist + drop-root — the production backend, jail-proven by a forked self-test (out-of-worktree reads denied, `unshare(2)` blocked); **macOS** applies a best-effort `sandbox_init` filesystem profile (local testing). Network stays open (the coder needs the LLM), and the child inherits provider/NATS env keys — so `--serve` stays trusted-nodes-only. Plus: the interactive TUI now offloads fan-out coders to the fleet (`FederationExecutor` wired in), presence heartbeats carry node identity, and a read-only `/fleet` command lists the remote swarm.

### Performance
- **Concurrent coders on a shared base (federation, Slice 1).** A `--serve` worker now runs up to `[federation] max_concurrent_coders` coders at once (default 4) instead of one at a time — they're model-wait-bound, so this multiplies throughput at little CPU cost. To keep concurrency from multiplying memory, all coders on a base commit share **one** materialized copy: a per-node cache holds one bare repo per base commit and each coder attaches a cheap detached git worktree off it (shared object store, not a full clone each). Pure optimization — a short deadline with an instant fall-back to a full clone, an in-use-guard so a live base is never evicted, and a `base = hit | miss | degraded` tag on each result.

### Migration
- The project moved to the **`entropy-om`** GitHub organization: `github.com/entropy-om/entheai`, tapped as `brew tap entropy-om/entheai`. The old `peterlodri-sec/entheai` URLs redirect.

## [0.2.1] - 2026-07-21

Interactive polish + a portable native-app fix, on top of the F2.1/F2.2
federation work landed since 0.2.0.

### Added
- `entheai --skills list` — list installed skills (name, description, path), the companion to `--skills add`.
- `entheai --skills remove <name>` — remove an installed skill by name (slugified → traversal-safe, scoped to the skills dir). Completes the add/list/remove surface.
- **Federation F2.1 — distributed swarm (opt-in `[federation]`).** New `entheai-federation` crate (JetStream work-queue + object-store git-bundles) + `entheai-worker --serve`/`--dispatch`: a coder task can run on another tailnet node — the dispatcher bundles the repo, enqueues a `WorkItem`, and applies the worker's delta to a `fed/…` branch; the worker pulls, materializes, runs the coder, and bundles the result back. Live-verified end-to-end.
- **Federation F2.2 — fan-out offload.** `entheai --fanout` now runs its coder sub-tasks on the fleet when `[federation]` is enabled and a worker is serving: a `CoderExecutor` seam in `run_fanout` (orchestrator stays NATS-agnostic — trait only), a worker **presence heartbeat** (`count_workers` gates dispatch), and a `FederationExecutor` that dispatches each coder and **squash-applies** the delta into its worktree so the existing commit/verify/integrate path is unchanged. Per-coder **local fallback** on no-worker/timeout/no-change; federation off → byte-identical to before. Executor path live-verified (presence + dispatch + squash-apply); full decompose→integrate offload wired (worker securefs hardening is F2.3).
- **Richer TUI slash surface** — a live `/`-menu (filter-as-you-type, `Tab` completes) now covers `/help`, `/clear`, `/fanout [on|off]`, `/model`, and `/quit`, alongside `/radio`, `/workers`, `/viz`.
- **Always-on env banner** — the status bar's second row shows the current + starting folder, a hostname-seeded machine id, and the primary local IP.
- **Token / context readout** — top-right `ctx ~cur/max · pct% · ↓out` on the status bar.

### Changed
- **`Esc Esc` stops the in-flight run; `Ctrl-C ×2` quits** (first press arms + shows a hint). A single `Esc` no longer quits.
- **`entheai --app` roots the window in the invocation cwd.** Ghostty's macOS login-shell wrapper reset cwd to `$HOME`, which hid the project's `.env` (empty provider key → 401) and pointed the agent at the wrong tree; the launcher now wraps the command in `sh -c 'cd <cwd> && exec …'`.
- **Default `max_turns` raised to 200**, and **unlimited under `--yolo`**.
- **Calmer companion pulse** — glow-breath periods slowed ~1.7× (idle 3.0→5.0s, working 1.5→2.5s).
- **Text-aware rain shader** — the raindrops refract only the empty background; glyphs and a small margin around them stay crisp.

### Fixed
- **Federation security-review pass** — same-host-only redirect guard on skill sub-fetches (SSRF), a 128 MiB git-bundle cap, redacted NATS URLs in logs, and `git reset/clean` cleanup when a squash-apply conflicts.

### Performance
- TUI history renders only the viewport slice (O(scrollback) → O(viewport) per frame).
- Viz swarm paint clones only `(status, short role)` per node, not each node's full task string.

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

[Unreleased]: https://github.com/entropy-om/entheai/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/entropy-om/entheai/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/entropy-om/entheai/releases/tag/v0.1.0
