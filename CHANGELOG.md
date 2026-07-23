# Changelog

All notable changes to `entheai` are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/); versioning: strict
[SemVer](https://semver.org/) — see [VERSIONING.md](VERSIONING.md).

## [Unreleased]

## [0.6.0] - 2026-07-23

The Dyad learning loop closes: failures become soil, outcomes reweight the
frozen priors, and the system's past mistakes reorder its future attention.

### Added
- **Dynamic frozen-node re-ranking (roadmap Phase 3.2).** Frozen node priors are now experience-weighted: a fan-out verify **failure** applies `rank −0.05` to every node whose triggers match the task + traceback (failures teach harder), and a **sealed success** applies `rank +0.02` to task-matched nodes via the new `TrajectorySink::ingest_sealed_success` hook (default no-op). Ranks clamp to `[0, 2.0]` and live in a persistent overlay (`frozen-ranks.json`, stored next to the PP raw store) that `FrozenStore::wake` consults in place of the static front-matter prior — so past mistakes reorder node selection across sessions without ever rewriting the frozen `.md` doctrine files.
- **Failure-trajectory auto-ingestion (roadmap Phase 3.1, first half).** When a fan-out coder's verify run fails (build, clippy, or test), the orchestrator now feeds the **full raw traceback** — not the 500-char display tail — to a new `TrajectorySink` seam, implemented by the prompt-processing memory: `PromptProcessor::ingest_failure_trajectory` stores it as `RawKind::Trajectory` under the `trajectories` namespace, capped and content-addressed (identical failures dedupe to one span). Wired in both the TUI and the one-shot `--fanout` CLI path whenever PP memory is on; best-effort, never blocks the fan-out.

## [0.5.0] - 2026-07-23

Structural honesty lands in the merge path: fan-out integration now demands
empirical verification and seals every merged diff to the log that earned it.

### Added
- **Mandatory deterministic merge seals for fan-out integration (roadmap Phase 2.1).** Every coder worktree merge now passes an empirical verification gate before integrating: `[fanout].verify` when set, else auto-detected `./scripts/check.sh` at the repo root. A passing run produces a deterministic **SHA-256 `MergeSeal`** — `sha256(diff)`, `sha256(verify log)`, and a combined seal over both — carried on the coder's outcome and printed in the fan-out report (`integrated ✓ — seal <12-hex> (verify: <cmd>)`). Self-reported coder success without empirical verification no longer integrates (enforcing `frozen/verification.md`).

### Changed
- **`[fanout].verify_required` (default `true`).** When no verify command resolves — neither `[fanout].verify` nor `./scripts/check.sh` — changed branches are now left unmerged for human review (`VerifyStatus::Unverifiable`) instead of integrating unverified. Set `verify_required = false` to restore the legacy integrate-as-is behaviour; such merges are loudly labelled `UNVERIFIED` in the report.
- **Worker sandbox toolchain grant follows "a verify could run".** `entheai-worker`'s Landlock read-only allow-list now includes `~/.cargo`/`~/.rustup` whenever verification is mandatory (the default) — not only when `[fanout].verify` is explicitly configured — since `./scripts/check.sh` may be auto-detected per-worktree.

## [0.4.0] - 2026-07-23

Voice output, an idle-aware brain panel, and a fully self-contained radio —
the binary no longer shells out to or fetches anything for playback.

### Added
- **`/speak` — read assistant responses aloud.** A new `entheai-tts` crate wraps the OS-native TTS engine (AVSpeechSynthesizer/NSSpeechSynthesizer on macOS via the `tts` crate) — no models, no network fetch, no external tool. Off by default; `/speak` toggles it, `/speak on`/`/speak off` set it explicitly, `/speak stop` interrupts mid-utterance. Speaks the full assistant answer once a turn completes (not per streamed token).
- **Brain panel reacts to real idle time.** A direct `user-idle` poll (~5s, the same sensor `rmcp-sensors`' idle tool wraps — already configured as an MCP server in `entheai.toml`) feeds `BrainState`, which now slows the faculties graph's rotation as the user steps away (linear falloff from full speed at <30s idle to a 0.15x floor at 5min+) and snaps back to full speed the moment they return. Bypasses MCP entirely (same direct-poll pattern as the existing NATS/Osaurus footer indicators) since MCP tools only fire on agent-initiated calls, not a continuous background signal. Gated behind the `desktop` feature — headless builds see `idle_seconds: None` and run at full speed, unchanged.

### Removed
- **`yt-dlp` dependency and arbitrary-URL/local-file radio playback.** `/radio add <url>`, `/radio <url_or_path>`, `/radio seed [pattern]`, and the `~/Downloads/Mesa*`-style genre-glob seed discovery are gone, along with the `[radio] download_timeout_secs` config key. `entheai-radio` no longer shells out to anything or touches the filesystem for content.

### Changed
- **Radio plays one bundled track, always.** "Standing-Onde" by 8bit-Wraith (<https://soundcloud.com/8bit-wraith/standing-onde>) is embedded in the binary at compile time (`include_bytes!`) and loops indefinitely — no network fetch, no cache directory, no install step (`yt-dlp` is no longer a dependency at all). `/radio` now only supports `pause`, `next` (restart the loop from the beginning), and `stop`.

## [0.3.0] - 2026-07-23

The BRAIN v1 slice: prompt-processing memory, frozen nodes, and a proactive
relevance judge, plus federation's sandboxed-worker hardening and fleet
visibility, an `adk-rust`-backed agent engine swap, and a round of interactive
TUI polish.

### Added
- **Permission posture + mode (`Shift+Tab` in TUI, `[permission] mode` in config).** Added a runtime permission posture `[plan · auto · yolo · ask]` cycled with `Shift+Tab` in the TUI, tool risk tiers (`Read < Write < Exec < Network < Spawn`), per-tool pins (`always_allow`, `always_ask`, `never`), and subagent tier-ceiling policy propagation (`ENTHEAI_MODE`).
- **Prompt-processing retrieval — Slice 1 (opt-in, `[memory] mode = "prompt-processing"`).** A new raw experiential tier (`crates/memory-pp`): full session transcripts and all tool outputs are captured RAW, content-addressed (idempotent), and retention-pruned. Retrieval runs recall → mesh re-rank → deterministic compression, but the mesh (`ultra-graph`) and compressor (`marqant`) are in-process stubs behind a strict overall deadline in Slice 1 — so retrieval always falls back cleanly to today's top-K, byte-identical, whenever PP is off, empty, erroring, or slow. Default `topk` behaviour is unchanged. Zero changes to `crates/memory`.
- **Prompt-processing — Slice 2 (the real subprocess seams).** The mesh and compressor stubs are replaced by real, process-isolated backends dropped into the *same* trait seams (no upstream change): a stdio-JSON-RPC **mesh sidecar** (`sidecars/ultragraph/serve.py`, `[memory.prompt_processing] sidecar_cmd`) that ranks candidate raw spans — via the user's `ultragraph` 1-bit mesh when importable, else a deterministic lexical reference scorer — and returns **ids only** (the Rust side rehydrates the never-rewritten raw), and a **`mq compress --semantic`** compressor (`marqant_cmd`) with file-arg I/O. Both are bounded by the pipeline's overall deadline, `kill_on_drop`, and capped readers; any spawn/protocol/timeout/missing-tool failure degrades to top-K, so PP is safe even without `python3`/`mq` installed. Set either command to `""` to force the in-process stub.
- **`entheai-ultragraph` — native Rust port of ultra-graph's 1-bit (ternary) inference core.** A new pure-`std` crate porting the deployed-inference path of the user's `ultra-graph` package (BitNet-b1.58 style): the ternary/int8 quantizers, base-3 5-per-byte ternary packing, the byte tokenizer, and the `.ugm` v1 binary loader + topological forward interpreter (dense trees, plain/residual ultra-edges). Verified byte-exact against the Python reference via a committed conformance fixture. Produced through the **recursive-development path** (implemented by `agy` on the user's Ultra models, then independently verified). Inference-only — training/autograd stay in Python.
- **Prompt-processing — native in-process mesh (default).** A `NativeMesh` re-ranks candidate raw spans **in-process with no subprocess** (`[memory.prompt_processing] mesh_backend = "native"`, now the default), dropping the Python sidecar from the default path while keeping it available (`mesh_backend = "sidecar"`). It scores each candidate with an optional `.ugm` reranker (`native_model`, run via `entheai-ultragraph`) over a `FEATURE_DIM=768` feature vector `[query hist | text hist | query⊙text interaction]` — the interaction block is what lets a linear ternary model rank a query×text match — or a deterministic lexical fallback. PP now completes end-to-end — recall → native re-rank → rehydrate raw → compress — with zero external tools.
- **Prompt-processing — a trained ternary `.ugm` reranker (`native_model`).** Ships a real BitNet-b1.58 reranker (`crates/memory-pp/models/reranker.ugm`) + its reproducible training pipeline (`tools/train_reranker.py`): a single dense ternary linear over the 768-d feature, trained with a margin ranking loss + straight-through estimator on synthetic topical triples, exported to `.ugm`. **94.7%** held-out ranking accuracy; the native mesh loads and runs it in-process (Rust acceptance test). Closes the ultra-graph → PP loop; produced through the recursive-development path (trained by `agy`, verified here). Optional (default stays lexical) — a v0 reference model; retrain with real relevance data via `train_reranker.py`.
- **Deterministic must-keep override for prompt-processing compression (`kompress-core` Mechanism B).** The compression path behind the `mq compress --semantic` / native compressor (`crates/kompress-core`) now force-keeps content the soft asymmetric-loss score alone might prune: ALLCAPS identifiers and signal names, CamelCase identifiers, hex addresses, exit-code-style bare numbers, CLI flags, and dotted filenames. It's a hard, score-independent override checked ahead of the existing loss/threshold gate — conservative by construction (from the kompress-v8 paper, "Asymmetric Loss Modulation Resolves the Voting Ensemble Paradox in Learned Context-Pruning Ensembles"): it can only prevent a prune, never cause one, so compressed retrieval context keeps the debugging-critical tokens a fidelity score alone would otherwise drop.
- **Brain panel (TUI).** An always-on compact side panel beside the chat: a slowly rotating braille pseudo-3D node graph of the agent's faculties (model · tools · context) and the remote fleet, with a live `wk N · nats ●/○ · ctx %` footer. Faculties flare on token generation / tool calls and decay; the fleet ring + NATS indicator come from a throttled 1.5 s presence poll. Toggle with `/brain`, config `[viz] brain` / `brain_width`; auto-hides on narrow terminals. (Slice B — a kitty-graphics true-3D upgrade behind `graphics_capable()` — is a planned follow-on.)
- **Frozen nodes — library (Slice 1, opt-in `[frozen]`).** Curated best-practice "frozen nodes" (`frozen/*.md`: TOML front-matter — name, domain, triggers, optional MCP, rank — + a distilled knowledge body) that sit dormant and **wake** when a task's *deterministic* triggers match, ordered by lexical relevance + rank. A woken node is distilled through marqant into a bounded, transient brief (the ice-in-coca-cola property — melts in, re-freezes, never persisted). `crates/memory-pp::frozen` (`FrozenNode`/`FrozenStore`/`wake`/`activate`) + a brain-panel frozen ring (`wake_frozen`) + `[frozen] enabled`(default false)`/dir/top_k/max_inject_bytes`. Ships 11 seeded, Valyu-researched nodes (nixos · terraform · docker · postgres · observability · rust · go-parallelism · python-jit · github · ngrok · valyu). Prompt-assembly wiring is now complete — `FrozenStore::wake` fires on every turn, one-shot and interactive alike, the woken brief is injected ahead of the user message, and an `AgentEvent::FrozenWoke` reaches the TUI — closing out Slice 1; Docker-MCP auto-load + experience-fed re-ranking remain Slices 2–3.
- **5 more frozen nodes, beyond the original 11.** `verification` (never trust a subagent's or remote worker's self-reported success — require running the local check script and reading raw exit codes/logs before merging), `coordinates-as-interface` (a coordinate system or data representation is an observer-chosen lens, not a claim about the underlying thing's true shape — suspect the representation before the data), `epistemic-reduction` (relevance/salience judging — rerankers, proactive judges, context pruners — is epistemic reduction: irreducibly approximate by nature, not a defect to engineer away), `memory-as-salience-not-fidelity` (a memory system should be judged by the salience it preserves for the agent's current self, not fidelity to the literal past — reframing spillover/pruning/compression as doing the job right), and `prediction-error-learning` (a relevance judge like BrainJudge should close a predict → compare → update loop on its own misses and false positives, not run forever as a static one-shot prompt).
- **Interactive TUI now runs the full BRAIN v1 memory/PP/BrainJudge stack (previously one-shot-only).** `run()`/`event_loop()` gained `memory`/`pp`/`scope` parameters, and `bin/entheai`'s interactive arm now builds the memory runtime, prompt-processor, and a fresh per-turn `MemoryScope` the same way the one-shot path already did — every interactive turn hits memory-pp's raw store and `FrozenStore::wake` retrieval, not just `entheai --fanout` runs. A second, lightweight `BrainJudge` (built from the same provider/model resolution as the main agent, no new config surface) also runs continuously in the interactive loop: it's notified on every `ToolFinished` event and drains its channel each tick, waking the brain-panel ring for proactive frozen-node matches independent of whether a task is in-flight.
- **Osaurus local-model status (TUI).** The status bar shows whether the local Osaurus endpoint (the `osaurus` provider's `base_url`, default `http://127.0.0.1:1337/v1`) is up and how many models it serves — green `osaurus ● 3` when reachable, dim `osaurus ○` when not. A throttled 5 s background probe (`GET /models`, 600 ms timeout, fail-safe) updates it off the render path, so a slow/absent endpoint never stalls a frame. Built via the recursive-development path (agy).
- **Automatic Pomodoro timer (TUI).** The status bar now carries an always-on, pure-ASCII 25-min-work / 5-min-break Pomodoro (`WORK 24:59` green, `BREAK 04:12` cyan) that cycles from launch with no command needed. It's a pure wall-clock model in `crates/viz` (`Pomodoro::at(elapsed)`), so it tracks real minutes; an idle session repaints it at ~1 Hz only when the countdown digit changes (no per-frame idle cost).
- **A welcome banner on TUI launch.** The chat opens with an assistant-style message summarizing what's online (swarm & model-matched workspace coders) and pointing at the most useful slash commands (`/help`, `/radio`, `/clear`, `/fanout`) along with the current fan-out on/off state.
- **Arrow-key navigation in the slash-command menu.** `Up`/`Down` now cycles the highlighted match in the `/`-menu (wrapping at the ends), `Right` or `Enter` accepts the highlighted command, and `Left` clears the selection back to free typing; `Tab` still completes an unambiguous single match as before.
- **`/config` — interactive configuration menu, plus an animated "thinking…" spinner.** `/config` opens an arrow-key-navigable modal (mode, fan-out, brain panel, swarm viz, model backend, a read-only Osaurus status line, a radio toggle, and close) toggled the same way as `/setup` below. Alongside it, the status bar's "thinking" indicator during a run gained an animated ellipsis (cycling 0–3 dots) with an alternating magenta/cyan spinner color, replacing the previous static label.
- **`/setup` — first-time interactive setup wizard.** A 5-step modal (arrow keys to move, `Left`/`Right`/`Enter` to change) for model backend, permission mode, brain-panel visibility, and fan-out, ending in a "Save Configuration & Finish Setup" step; settings apply to the running session.
- **Procedural ambient radio (`/radio procedural`, `/radio seed [pattern]`).** The in-TUI radio player can now loop local audio instead of only fetching URLs via `yt-dlp`: `/radio procedural` (or `/radio seed [pattern]`, default `~/Downloads/Mesa*`) scans a directory or glob — matching an explicit prefix or, by default, filenames containing `mesa`/`desert`/`psychedelic`/`stoner`/`space`/`metal`/`chillout` — and falls back to `~/.cache/entheai/radio/` if nothing matches there. When the queue drains, it pseudo-randomly rotates through the seed files (`♪ Procedural <title> (Variation #N)`) instead of going silent, so ambient playback never just stops. `/radio <path>` also now accepts local files, not just `http` URLs.
- **Federation F2.3 — worker confinement + fleet visibility.** Each coder now runs in a self-sandboxing `entheai-worker --sandbox-run` child (new `entheai-sandbox` crate), governed by `[federation] sandbox = "strict" | "permissive" | "off"` (default `permissive`): **Linux** applies a Landlock filesystem jail + seccomp syscall denylist + drop-root — the production backend, jail-proven by a forked self-test (out-of-worktree reads denied, `unshare(2)` blocked); **macOS** applies a best-effort `sandbox_init` filesystem profile (local testing). Network stays open (the coder needs the LLM), and the child inherits provider/NATS env keys — so `--serve` stays trusted-nodes-only. Plus: the interactive TUI now offloads fan-out coders to the fleet (`FederationExecutor` wired in), presence heartbeats carry node identity, and a read-only `/fleet` command lists the remote swarm.

### Changed
- **Internal engine swap: `crates/core`'s agent loop now runs on `adk-rust` (behavior-preserving).** The hand-rolled `Agent<P>` / provider-loop / `run_task` / `run_task_with_memory` system is replaced end-to-end by an `adk-rust`-backed `EntheaiAgent` (`LlmAgent` + `Runner`, `before_model`/`after_model` callbacks for memory retrieval and frozen-node wake, an `event_bridge` driving `adk-rust`'s event stream into `AgentEvent`), and the now-fully-superseded `crates/providers` crate is deleted outright — `entheai-providers` no longer exists as a workspace crate or dependency. The migration targeted, and hit, zero observable behavior change (460 tests passing, clippy-clean; the six parity tests were ported against the new system, with two intentionally-updated assertions where the underlying `adk-rust` library's own error wording / JSON-parsing behavior differs from the old code, documented in `crates/core/tests/parity.rs`). One thing users can actually hit: the workspace **MSRV rises from `1.80` to `1.94`** (`rust-version` in `Cargo.toml`) to satisfy the new dependency — a toolchain older than 1.94 can no longer build `entheai` from source.

### Fixed
- **Status bar / swarm visualization polish.** The status bar's left status line and right-aligned context readout (`ctx ~cur/max · pct% · ↓out`) could silently overlap and garble each other on a standard 80-column terminal once mode/model/pomodoro/osaurus text pushed the left line past ~76 columns — the left line is now width-capped, dropping whole trailing segments rather than truncating mid-word. In the swarm view, the running-node glyph no longer strobes at the ~11 Hz animation-tick rate (held for 4 ticks, ~360 ms, so it reads as a pulse rather than a flicker); node/orchestrator labels are now explicitly styled with their status color instead of relying on an incidental color bleed from neighboring cells; and the full-screen swarm view (`Ctrl-V`) shows a centered hint instead of a bare "orch" hub when idle.

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
