# entheai — Hybrid, Visual, Self-Improving Coding Agent Harness

**Design spec** · 2026-07-18 · status: approved — ready for v0.1 implementation planning

---

## 1. Summary

`entheai` is a personal, **macOS / Apple-Silicon-only**, terminal-based (TUI) coding-agent harness written in **Rust**, with **visualization as a first-class identity** (shader backgrounds + a live codebase graph). Its defining idea is a **tiered hybrid brain**: a strong cloud orchestrator (DeepSeek V4 Pro) plans and decomposes work, and when implementation effort crosses a threshold it **fans out** to a heterogeneous set of sub-agents, each matched to the **best model for its task**. The system **learns** from real build/test outcomes, treats deep knowledge of *your* codebase as always-on, pools compute across your own machines over a **Tailscale tailnet**, and continuously **feeds a self-improvement data flywheel** (`dogfeed`).

Because the main tool targets only M-series Macs, we specialize hard (Metal, MLX, Seatbelt, terminal graphics) with no portability tax. Only the headless *worker* and *inference* peers are cross-platform.

It is **deeply extensible** via a clean four-way model — native tools · **skills** (Claude-Code-compatible instruction packs; superpowers/caveman/BMAD bundled) · **plugins** (managed external CLIs, auto-provisioned) · MCP servers — and ships an always-on local **crash/health sidecar** (`Sonar`). Performance is a first-class development value (§9).

**Synthesis of ideas (reimplemented in Rust; none merged as code — references or bundled sidecars):**
- **Crush** (Go) — TUI/UX feel + YOLO permission model (inspiration only; FSL → no code reuse).
- **CodeWhale** (Rust, MIT, DeepSeek-origin) — richest reference: durable resumable runs, `execpolicy` sandbox, provider route-resolution, context spillover, side-git snapshots.
- **Ruflo** (TS, MIT) — hierarchical sub-agents, memory-first coordination, ReasoningBank/trajectory learning, HNSW memory, federation.
- **codebase-memory-mcp** (C, MIT) — bundled MCP sidecar: code knowledge graph + 3D graph web UI.
- **Osaurus** (Swift, macOS/MLX) — local inference server (HTTP sidecar).
- **dogfeed / ultrawhale-dogfood** (user's own; TS/Bun canonical, MIT / dataset Apache-2.0) — self-improvement flywheel feeding a Hugging Face dataset.
- **Honcho** (Plastic Labs, AGPL-3.0) — user-modeling/personalization service (Dialectic API); built-in as a managed local sidecar.

## 2. Goals & non-goals

**Goals**
- Daily-usable personal coding agent optimized for capability, speed, control — and *look*.
- Hybrid model brain: cloud orchestrator + local + cheap-cloud workers, matched per task.
- Fan-out orchestration with per-sub-agent model selection + isolated parallel writes.
- Two-tier, five-namespace persistent memory that compounds on your codebases; 100% local, no API keys.
- Self-learning loop that tunes routing/planning from real outcomes.
- **Self-improvement flywheel** (`dogfeed`): feed real agent trajectories into the HF dataset, low overhead.
- **User modeling / personalization** (`honcho`): a built-in layer that models who the user is (preferences, working style) and personalizes the orchestrator.
- Built-in codebase understanding via a bundled MCP.
- Visual identity: toggleable shader backgrounds + a toggleable live codebase graph.
- **First-class skills** (discover, invoke, create) — Claude-Code-format-compatible; superpowers/caveman/BMAD bundled.
- **Managed CLI plugins** auto-provisioned at install (detect → confirm → install/upgrade).
- Always-on local **crash/health sidecar** (`Sonar`) for the app + all sub-agents.
- Federation across your own machines over a Tailscale tailnet.
- Deep macOS/Apple-Silicon specialization; performance-first engineering.

**Non-goals (YAGNI)**
- Cross-platform **main CLI/TUI/orchestrator**. macOS 15.5+/M-series only. (Headless worker + inference peers may be any OS.)
- Full swarm consensus (Byzantine/Raft/Gossip).
- Multi-org / cross-org federation. Only the user's own tailnet.
- A distributable product (onboarding, marketplace, broad docs).
- Replicating dogfeed's *synthetic* Q&A-generation loop — entheai only contributes *real* trajectories (§5.18).

## 3. Constraints & context

- **Main binary (`entheai`):** macOS 15.5+, Apple Silicon only. Uses Metal/MLX-adjacent, Seatbelt, and the **Kitty graphics protocol** → needs a graphics-capable terminal (**Ghostty / Kitty / WezTerm**); degrades gracefully elsewhere.
- **Worker (`entheai-worker`):** portable, headless (no TUI/viz) — runs the agent loop + tools + comms so any-OS peers can execute dispatched sub-agents.
- **Sidecar (`entheai-sonar`):** local crash/health monitor + minimal UI.
- **External processes:** Osaurus (`127.0.0.1:1337`), bundled **codebase-memory-mcp** (MCP + 3D graph UI on `:9749`).
- **Cloud providers (OpenAI-compatible):** OpenCode Zen (primary), DeepSeek direct, OpenRouter.
- **Federation:** Tailscale (primary) / ZeroTier (alt) / manual (fallback).
- **Plugin installer:** Homebrew (primary on macOS).
- **dogfeed sink:** HF dataset `PeetPedro/ultrawhale-dogfood` (gated, Apache-2.0); push opt-in via `HF_TOKEN`/`HF_REPO`.
- **Perf bible:** Lattimore, "Wild performance tricks" + other Rust perf refs (§9).
- **License posture:** references MIT (CodeWhale, codebase-memory-mcp, Osaurus, Ruflo, dogfeed pkg). Crush FSL — inspiration only.

## 4. Architecture overview

One Rust workspace, three binaries: `entheai` (macOS, full), `entheai-worker` (portable, headless), `entheai-sonar` (local crash/health UI).

```
┌───────────────────────────────────────────────────────────────┐
│  entheai · macOS/M-series TUI binary                            │
│   tui (ratatui) + viz (wgpu shaders → Kitty graphics)           │
│   core loop ─▶ router/model-selection ─▶ agents (fan-out)       │
│     · executor seam: Local(tokio) | Remote(tailnet node)        │
│     · per-coder git worktree → join/reduce (build+test)         │
│   memory(2 tiers/5 ns) · learning(trajectories) ─┐              │
│   extensions: tools · skills · plugins(CLIs) · mcp │ event bus  │
│   permission+YOLO · sandbox(Seatbelt) · session   │            │
│   dogfeed exporter ◀──────────────────────────────┘            │
│   comms(Tailscale) ─┐   panic/health reporter ─┐               │
└──────────┬──────────┼──────────────┬───────────┼──────────────┘
           │          │              │           │
 ┌─────────▼──┐ ┌─────▼─────┐ ┌──────▼───────┐ ┌─▼──────────────┐
 │ Osaurus    │ │ Cloud     │ │ codebase-    │ │ entheai-sonar  │
 │ local +    │ │ Zen/DS/OR │ │ memory-mcp   │ │ crash/health   │
 │ embeddings │ │           │ │ + graph:9749 │ │ local UI       │
 └────────────┘ └───────────┘ └──────────────┘ └────────────────┘
      ▲              │
 ┌────┴───────────┐  ▼ (opt-in)
 │ tailnet peers  │ ┌──────────────────────────┐
 │ (any OS):      │ │ HF dataset               │
 │ worker /       │ │ PeetPedro/ultrawhale-    │
 │ exposed Osaurus│ │ dogfood  (dogfeed sink)  │
 └────────────────┘ └──────────────────────────┘
```

### Crate / module map

Portable crates compile for the worker; macOS-only crates marked ⌘.

| Crate | Responsibility |
|---|---|
| `core` | Agent loop, orchestration, session/message types; the trajectory **event bus**. |
| `providers` | OpenAI-compatible client + registry; live catalog. Zen/DeepSeek/OpenRouter/Osaurus; pluggable. |
| `router` | Model-selection layer: `(provider, model, node)` per node; fallback; learning-aware. |
| `agents` | Declarative roles; fan-out planning; executor seam (Local/Remote); worktree isolation; join/reduce. |
| `memory` | Unified `Memory` trait over 2 tiers / 5 namespaces; adaptive flat↔HNSW; SQLite + Osaurus embeddings. |
| `learning` | Trajectory capture/retrieve/score → tunes routing (ReasoningBank essence). |
| `dogfeed` | Self-improvement exporter: agent events → schema → PII-scrub → batch → HF dataset push. |
| `compaction` | Automatic context compaction: `kompress-v8` ONNX token-pruning in-process (`ort`+`tokenizers`), must-keep override. |
| `honcho` | User-modeling/personalization: REST client + Apple-container sidecar supervisor for Honcho (Dialectic API); deriver → our tier. |
| `mcp` | MCP client + supervisor (codebase-memory-mcp bundled). |
| `skills` | Skill discovery/registry/invocation/creation; Claude-Code Agent-Skills format; bundles superpowers/caveman/BMAD. |
| `plugins` | Managed external CLIs: probe/version-detect, confirm-gated brew install/upgrade, expose as tools. |
| `tools` | fs/`apply_patch`/shell/search; large outputs spill to `tools` memory. |
| `permission` | Blocking gate; `--yolo`; allowlist; auto-approve-session. |
| `sandbox` ⌘ | macOS Seatbelt wrapping tool/shell exec. |
| `session` | SQLite persistence; durable, crash-resumable ledger (Fleet-inspired). |
| `comms` | Tailscale tailnet transport: peer discovery, MagicDNS, remote-inference & remote-execution. |
| `health` | Panic hook + supervision reporter; streams events to Sonar over a local socket. |
| `tui` ⌘ | ratatui UI: chat/streaming, diff view, permission dialog, session picker, model/tier/node indicator. |
| `viz` ⌘ | wgpu offscreen shaders → Kitty graphics background; built-in shaders; graph toggle (`:9749`). |
| `config` | TOML: providers, keys, roles, router policy, ui/shaders, tailnet nodes, skills, plugins, dogfeed, sonar. |

**Binaries:** `entheai` ⌘ (all) · `entheai-worker` (portable subset: core/providers/router/agents/memory/learning/dogfeed/mcp/skills/plugins/tools/comms/session/health/config) · `entheai-sonar` (crash/health UI).

## 5. Component design

**5.0 Extension taxonomy.** entheai extends four composable ways: **native tools** (built-in Rust), **skills** (markdown packs that guide the model), **plugins** (managed external CLIs), **MCP servers** (external tool/resource servers). A skill may instruct use of a plugin CLI or MCP tool; a sub-agent role may bind specific skills.

### 5.1 core — agent loop
Streaming loop: assemble prompt (system + memory context + history) → call model → stream → dispatch tools → feed results → repeat. Orchestrator turn emits a **plan** with per-item **effort scores**. Publishes every step to the **trajectory event bus** consumed by `learning` and `dogfeed`.

### 5.2 providers — registry
One OpenAI-compatible client covers Zen/DeepSeek/OpenRouter/Osaurus. Registry resolves logical id → route (base URL, auth, protocol, context, price). **Live catalog** via `/v1/models` (deprecation dates → never hardcode). Primary door **OpenCode Zen** (`https://opencode.ai/zen/v1`): DeepSeek V4 Pro/Flash, Qwen 3.7, GLM, Kimi, free models via one key. Honest cost display.

### 5.3 router — model-selection layer
Per node: role/task-type, effort, context size, cost/latency budget, privacy, provider/node availability → `(provider, model, node)` + fallback chain. Declarative `role→[models]` + rules; **effort gate** (< threshold → inline); **node placement**; **learning-aware** (v0.2+) blends static config with learned win-rates.

### 5.4 agents — fan-out orchestration
Declarative roles (file-per-role + frontmatter): prompt, tools, model preference+fallback, effort profile, **bound skills**. Built-ins: `explore` (drives codebase-memory graph tools), `coder`, `docs`, `test`, `review`, `merge`. Fan-out plan = DAG of `{role, task, effort, model, deps}`. **Executor seam** `Local(tokio)` (v0.1) / `Remote(node)` (v0.3). Each `coder` in its own **git worktree** → patch. **Join/reduce:** apply patches; conflict → `merge` agent/orchestrator; **build + tests**; integrate (or diff for approval unless `--yolo`). Promote residue to `learnings`; capture trajectory.

### 5.5 memory — two tiers, five namespaces, one engine
`Memory` trait; each call names a namespace. `codebase` federates to the MCP; rest = local SQLite + vector (Osaurus embeddings). No API keys.

| Namespace | Tier | Owns |
|---|---|---|
| `codebase` | long-term | repo structure: symbols, call graph, architecture, ADRs (external MCP) |
| `learnings` | long-term | durable facts, preferences, "how we solved X" |
| `trajectories` | long-term | reasoning paths + outcomes + scores (ReasoningBank) |
| `tools` | working | tool results; large outputs spilled & recalled; success/fail signals |
| `subagents` | working | per-sub-agent scratch + outputs; orchestrator↔worker bus |

**Adaptive indexing:** exact flat search below the ANN crossover (~5k vectors, per AgentDB benchmark), auto-HNSW above. Impl: vendor AgentDB Rust core or `instant-distance`/`usearch` + SQLite-WAL.

### 5.6 learning — self-learning loop
Capture each run's trajectory → retrieve similar ones pre-task (inject "what worked/failed") → score post-task, reinforcing winning `(role, task-shape) → model/decomposition` patterns and decaying losers. The router self-tunes model-matching from evidence.

### 5.7 mcp — client + supervisor
MCP client (stdio+http) + supervisor spawning bundled servers. **codebase-memory-mcp** bundled, auto-started, auto-`index_repository` on open; `explore`+orchestrator prefer its graph tools over grep; ships local `nomic-embed-code`; its **3D graph UI (`:9749`)** is what the viz graph toggle surfaces. External user MCP servers supported.

### 5.8 tools
`read_file`/`write_file`/`apply_patch`/`run_shell`/`search`. Structured over raw shell. Large outputs spill to `tools` namespace, recalled on demand. Credential redaction at wire boundary; `read_file` refuses credential paths.

### 5.9 permission + YOLO
Blocking `Service`: `request()` waits for TUI resolution. Tiered short-circuit (Crush): hook pre-approval → `--yolo` skip → allowlist → auto-approve-session → per-request grant. Worktree isolation + Seatbelt make YOLO safer.

### 5.10 sandbox ⌘
macOS Seatbelt profile (writes restricted to worktree; network per policy). Minimal v0.1 → full `execpolicy` v0.2. Worker: per-platform best-effort.

### 5.11 session
SQLite persistence → durable, append-only, **crash-resumable ledger** (Fleet-inspired); `resume` survives restarts.

### 5.12 comms — federation over the tailnet (v0.3)
Tailscale delegates identity/auth, encryption/NAT-traversal, and discovery to the tailnet. `comms` = *list peers (`tailscale status --json`/LocalAPI) → address by MagicDNS → dispatch.* **(a) remote inference** (provider entry → peer's exposed Osaurus; ships first); **(b) remote execution** (dispatch a sub-agent to `entheai-worker` on the peer, any OS). ZeroTier alt; manual fallback. Executor seam + node placement exist from v0.1.

### 5.13 tui ⌘
ratatui: single-session chat + streaming, syntax-highlighted **diff view**, **permission dialog**, session picker, live **model/tier/node** indicator. UX patterns from Crush.

### 5.14 viz ⌘ — visual identity
**Shader backgrounds:** wgpu renders a shader offscreen per frame → blit **behind text** via Kitty graphics protocol (negative z-index). Toggleable; built-ins **`RandomShader`** + **`Cyberpunk`** (+ user-shader slot). Graceful fallback (shaders off / animated truecolor gradient). **Graph toggle:** surfaces codebase-memory-mcp's **3D graph web UI (`:9749`)** in a webview/browser (native embed later). Frame-budgeted; pauses when idle (battery).

### 5.15 config
TOML (see §5.20 for the consolidated example).

### 5.16 skills
**Format:** Claude-Code **Agent Skills** convention — `SKILL.md` + frontmatter (`name`, `description`, when-to-use), optional bundled scripts/resources → the existing ecosystem drops in. **Discovery:** bundled dir + `~/.entheai/skills` + project `.entheai/skills`; registry indexes name+description for relevance surfacing. **Invocation:** a `Skill` tool the orchestrator/sub-agents call; content loads into context and is followed (process-skills vs. flexible). **Creation:** built-in skill-creator flow (author/edit/validate) — top priority. **Bundled:** **superpowers** (brainstorming, writing-plans, TDD, debugging…), **caveman** (compressed output), **BMAD** (BMAD-METHOD agentic agile). Roles can bind skills (§5.4).

### 5.17 plugins — managed CLIs
External CLI tools entheai provisions and exposes to the agent. **Manifest:** `{name, probe(cmd+version regex), min_version, install(brew|cargo|curl), expose}`. **Provisioning:** at setup + first-use, probe presence/version; if missing/outdated → **confirmation prompt showing the exact install command** → install/upgrade via **Homebrew** (primary). Never silent. **Catalog:** cloud CLIs — Hetzner `hcloud`, AWS `aws`, DigitalOcean `doctl`, Vultr `vultr-cli`, OVHcloud; utilities — GNU `coreutils`, `timeout`; scheduling — `cron`, loop runner. Extensible. Exposed as callable tools under the permission gate + sandbox. Worker peers provision per-platform.

### 5.18 dogfeed — self-improvement flywheel exporter
**Insight:** `dogfeed`/`ultrawhale-dogfood` currently *self-generates* topics — it lacks real agent data. **entheai is the real-data feeder it was missing**, and entheai's multi-model fan-out (DeepSeek *teacher* + free/local *students*) maps naturally onto dogfeed's generator/student/teacher three-model structure. entheai implements only the **exporter/feeder**, not dogfeed's synthetic loop.
- **Source:** subscribes to the same trajectory **event bus** as `learning` (one capture, two sinks).
- **Pipeline:** agent events (prompt, plan, tool_calls, diff, model, build/test outcome, score, session_id, loop_index) → map to dogfeed schema → **PII scrub** (email/phone/IP/API-key) + critical-token safety floor → dedup → batch (every N) → **async SQLite-WAL buffer off the hot path** → push to HF via `create_commit` (add op) → mirror `latest.jsonl`, refresh `stats.json`, optional webhook. Budget caps (calls/tokens/day).
- **Schema (from `publish.ts`):** `{id, topic, question, answer, compressed_answer, model, tokens_in, tokens_out, role, source:"entheai", topic_category, created_at}` + richer fields (`user_message, free_response, free_model, deepseek_response/reference, text, session_id, loop_index, pipeline, enriched_at`).
- **Privacy:** local capture always on (cheap); **remote push opt-in** via `HF_TOKEN`/`HF_REPO` (defaults on when creds present — it's the user's own dataset); PII scrub mandatory before any push.
- **Impl:** native Rust exporter (~200 lines) preferred over running dogfeed as a sidecar — minimal overhead.

### 5.19 compaction — automatic context compaction
When context approaches the window limit, entheai **automatically compacts** older history and large tool outputs using **`kompress-v8`** (`PeetPedro/kompress-v8`) — our own model. It is **not** a generative summarizer: it's a ~150M-param **token-classification** model doing **extractive per-token pruning** (~15% of low-value tokens dropped) with a **must-keep override** protecting critical tokens (paths, commands, secrets, tool outputs) exactly. Runs **in-process via ONNX Runtime** (`ort` crate) + `tokenizers` — *not* Osaurus/MLX, which serve generative models. Recent turns + active plan + open sub-agent state are preserved verbatim; older spans and big tool outputs are pruned, originals retrievable from the `tools`/session store. Composes with spillover (§5.8); for very old spans needing deeper reduction, an optional abstractive pass by a small generative model can follow. Optionally int8-quantize the ONNX for speed. Fallback: run the `headroom` proxy as a sidecar over HTTP. **This closes the flywheel:** `dogfeed` trajectories are the kompress family's training data; `kompress-v8` then compacts entheai's own context — cheaper, longer sessions that improve as the dataset grows.

### 5.20 Sonar — crash/health sidecar
Always-on **local-only** crash/health monitor + minimal UI (name *Sonar*, pencilled; alts Watchtower/Kennel). The `health` crate installs a **panic hook** + supervises sub-agents (local tokio tasks + remote tailnet workers), streaming liveness/crash/stack-trace/exit-code events over a local socket to **`entheai-sonar`**, which renders a minimal dashboard (app + every agent's status, recent crashes, logs). Serves both **using** entheai and **developing** it. No phone-home; low overhead.

### 5.21 honcho — user modeling & personalization
`honcho` (Plastic Labs) is a built-in **personalization layer** modeling *who the user is*, distinct from code/agent memory. A background "deriver" continually builds a reasoning **representation** of the user (preferences, working style, how they like explanations) from the conversation, queried via its **Dialectic API** (`POST /peers/{id}/chat`) — e.g. "how should I explain this to this user?" — whose answer is injected into the orchestrator's system prompt to personalize behavior over time.
- **Data model:** Workspace → Peers (the user, optionally sub-agents) → Sessions → Messages; derived Representations/Conclusions/Summaries. entheai ingests turns into a per-project (or global) session and reads the representation before planning.
- **Deployment:** NOT a single binary — Python/FastAPI + **Postgres/pgvector** + deriver worker. "Built-in" = entheai auto-provisions and supervises it as a **local sidecar via Apple Containerization** (`container` CLI/framework, macOS 26+) next to the main binary — **no Docker**, native Linux-container VMs on Apple Silicon (the same runtime Osaurus uses). Fallbacks: **hosted `api.honcho.dev`** (one HTTP dep) on macOS < 26 or by choice, and graceful **off** if neither is available. The `honcho` crate is a REST client + Apple-container sidecar supervisor.
- **Deriver LLM:** pointed at our own tier via `OVERRIDES__BASE_URL` (OpenAI-compatible) — local Osaurus or DeepSeek — so user-modeling stays on our providers. Every ingested message triggers (batched) LLM calls; cost/latency managed by batching + a cheap deriver model.
- **Relationship:** owns rich **user-modeling**, narrowing the `learnings` namespace to *task solutions* (Honcho = who the user is; learnings = how tasks were solved; codebase = the code; trajectories = reasoning paths).
- **License:** AGPL-3.0 — fine for personal use; a distribution/hosting caveat only if entheai is ever shipped to others (the hosted API sidesteps it).

### 5.22 config example
```toml
[router]
orchestrator = "zen/deepseek-v4-pro"
fanout_threshold = 5
max_parallel = 8
escalate_to = "zen/deepseek-v4-pro"

[agents.coder]
tools  = ["fs","shell","search","codebase"]
effort = "high"
model  = ["zen/deepseek-v4-flash","openrouter/qwen3.7-coder","osaurus/qwen3-coder"]
skills = ["superpowers/test-driven-development"]

[ui]
shader = "cyberpunk"        # off | random | cyberpunk | <custom>
shader_enabled = true
graph_toggle = true         # surfaces codebase-memory-mcp :9749

[skills]
bundled = ["superpowers","caveman","bmad"]
paths   = ["~/.entheai/skills",".entheai/skills"]

[plugins]
autoprovision = true         # detect → confirm → install/upgrade
installer = "brew"
enabled = ["coreutils","timeout","hcloud","doctl","aws","cron"]

[dogfeed]
enabled = true               # local capture always on
push = "auto"                # auto (if creds) | on | off
hf_repo = "PeetPedro/ultrawhale-dogfood"
pii_scrub = true

[compaction]
auto = true
model = "kompress-v8"        # ONNX token-classifier, run in-process (ort + tokenizers)
preserve_recent_turns = 6
must_keep = ["paths","commands","secrets","tool_output"]

[sonar]
enabled = true

[honcho]
enabled = true
mode = "local"                # local (Apple-container sidecar, macOS 26+) | hosted | off
deriver_model = "osaurus/qwen3"   # deriver LLM routed through our providers

[[nodes]]                    # federation over tailnet (v0.3)
name = "studio"
host = "studio.tailnet.ts.net"
osaurus = "http://studio.tailnet.ts.net:1337"
worker = true
```

## 6. End-to-end task lifecycle

1. Task entered in the TUI.
2. `core` builds context via **search-before** (`codebase`+`learnings`+`trajectories`).
3. Orchestrator plans; assigns effort scores.
4. **Effort gate:** below → inline; above → fan-out DAG.
5. `router` resolves `(provider, model, node)` per node.
6. Sub-agents run (Local/Remote), coders in worktrees, writing to `subagents`; tool outputs spill to `tools`. Every step → **event bus**.
7. **Join/reduce:** merge patches, resolve conflicts, build + test, integrate (or diff unless YOLO).
8. **Store-after:** promote to `learnings`; capture `trajectory`; `learning` scores routing; `dogfeed` batches + pushes (opt-in) to HF.
9. Session persisted; `health`/Sonar tracks liveness throughout.

## 7. v0.1 — "the spine lights up end-to-end"

Every layer in its thinnest real form.

- **tui + viz:** chat/streaming, diff view, permission dialog, status line; **one** shader + toggle + fallback; **graph toggle** → `:9749`.
- **core + tools:** working loop; fs/`apply_patch`/shell/search; **event bus** live.
- **providers:** Zen + Osaurus (OpenRouter/DeepSeek-direct → v0.2).
- **router:** explicit `role→model` + effort threshold (auto → v0.2).
- **agents/fan-out:** `explore` + `coder`; ≤N parallel coders in worktrees; join = merge + build/test. Seam Local-only.
- **memory:** all 5 namespaces minimal; adaptive index starts flat.
- **learning:** capture only.
- **mcp:** bundled codebase-memory-mcp, auto-index.
- **skills:** discovery + `Skill` invocation + bundled superpowers/caveman/BMAD (creation → v0.2).
- **plugins:** provisioning framework (probe→confirm→brew) + minimal set (coreutils, timeout) — full catalog → v0.2.
- **permission/YOLO:** gate + `--yolo` + allowlist.
- **session:** SQLite (durable ledger → v0.2).
- **sandbox:** minimal (gate + worktree isolation).
- **Sonar:** minimal — are the app + sub-agents alive/crashed.
- **compaction:** auto-compaction seam present; v0.1 uses simple truncation/summary fallback, `kompress-v8` wired in v0.2.
- **dogfeed / comms:** seams only (dogfeed capture buffered locally, no push; comms no networking) → v0.3.

**v0.1 acceptance:** on a real repo, a feature task → orchestrator plans, fans out ≥2 coders on local models into worktrees, merges + runs tests, integrates — with codebase-memory answering structure queries, one shader + graph toggle working, a skill invocable, Sonar showing agent health, and a trajectory captured.

## 8. Roadmap

| Version | Adds |
|---|---|
| **v0.1** | Thin spine (§7): loop + fan-out + memory(5) + codebase MCP + minimal viz + **skills + bundled packs** + plugin framework + **minimal Sonar**. |
| **v0.2** | Auto routing + learned matching; OpenRouter + DeepSeek-direct; Seatbelt `execpolicy`; durable ledger; roles docs/test/review/merge; richer DAG; **full shader library**; **skill creation**; **full plugin catalog**; **automatic compaction (kompress-v8)**. |
| **v0.3** | Full self-learning scoring/decay; hooks; **dogfeed exporter → HF**; **Tailscale federation** (remote inference → execution); **rich Sonar UI**; budget-aware routing. |
| **v0.4+** | Pluggable topologies; ADR surfacing; native embedded 3D graph; custom skills authoring UX; more providers; offline mode; **Honcho personalization** (user-modeling via Dialectic API, Apple-container sidecar). |
| **v1.0** | Config freeze, perf passes, personal docs. |

## 9. Development practices & performance

**Performance is a first-class value.** Canonical guide ("the bible"): David Lattimore, *"Wild performance tricks"* (`https://davidlattimore.github.io/posts/2025/09/02/rustforge-wild-performance-tricks.html`) + other Rust perf posts/books as designated. Fetch + index it before perf-sensitive work.
- **Guard the hot paths:** agent loop, wgpu→Kitty render loop, vector search, fan-out scheduler, memory I/O.
- **Practices:** minimize allocations on hot paths; prefer static dispatch where it matters; keep dogfeed/Sonar/memory-writes async + batched *off* the hot path; release builds with LTO + `codegen-units=1`; profile before optimizing; frame-budget the render loop and pause when idle.
- Enforced via TDD (superpowers) + code review.

## 10. Key decisions & rationale
- **Build fresh in Rust**; CodeWhale as the Rust/MIT reference.
- **macOS/M-series specialization** on the main binary; portability confined to `entheai-worker`.
- **In-process async sub-agents** behind a `Local | Remote` seam.
- **One memory engine, typed namespaces**; **one event bus, two sinks** (`learning` + `dogfeed`).
- **Bundle codebase-memory-mcp** (reuse its 3D graph UI); **Osaurus as HTTP sidecar**; **Zen as primary gateway**.
- **Claude-Code Agent-Skills format** so superpowers/caveman/BMAD drop in.
- **Homebrew, confirmation-gated plugin provisioning** ("comes with us", never silent).
- **dogfeed = native Rust exporter**, real trajectories only (not the synthetic loop).
- **Automatic compaction via our own `kompress-v8`** — an ONNX token-pruning classifier run in-process (`ort`; not Osaurus, since it's not generative), with a must-keep override for paths/commands/secrets. Closes the flywheel: dogfeed data trains kompress; kompress compacts entheai's context.
- **Honcho for user-modeling** — supervised **local sidecar via Apple Containerization** (macOS 26+, no Docker), deriver on our tier (private); hosted/off fallback; REST client. AGPL-3.0 acceptable for personal use.
- **Sonar = local-only crash/health sidecar** (no phone-home).
- **Federation over Tailscale**; **terminal + Kitty graphics** for visuals.
- **Performance-first** development (§9).

## 11. Risks & open questions
- **Thin-slice risk:** many subsystems at once → trivial per-layer v0.1 forms + a concrete acceptance test.
- **wgpu→Kitty compositing** is the riskiest v0.1 piece → prove with one shader early; clean off-path fallback; terminal dependency acceptable (macOS + personal).
- **Fan-out merge conflicts** → worktree isolation + `merge` agent + mandatory build/test gate; decomposition quality is the lever.
- **Auto-installing CLIs (plugins)** → strictly confirmation-gated, show exact command, sandbox execution, never silent.
- **dogfeed privacy** → local buffer default; remote push opt-in; PII scrub + safety floor mandatory; the HF dataset is gated. Dataset viewer is currently broken (parquet export) — treat as experimental.
- **Honcho footprint & license** → heavier than other built-ins (FastAPI + Postgres/pgvector + deriver); runs as an Apple-container sidecar needing **macOS 26+** (else hosted or off). AGPL-3.0 matters only if entheai is distributed; per-message deriver LLM cost mitigated by batching + a cheap model.
- **Model catalog drift** → always fetch `/v1/models`; alert on missing configured models.
- **codebase-memory-mcp, Osaurus, dogfeed are pre-1.0** → pin versions, isolate behind seams.
- **Skill-format drift** vs. Claude Code → pin to a documented convention version.
- **Open:** vector-store impl (vendor AgentDB vs. crate); no official Rust `tsnet` (LocalAPI/CLI vs. embed); worker sandboxing on non-macOS; dogfeed role-mapping fidelity (teacher/student) to the dataset schema.

## 12. References

| Project | Lang / License | Role | Borrowed |
|---|---|---|---|
| Crush | Go / FSL-1.1-MIT | inspiration | TUI/UX, tiered YOLO permission model |
| CodeWhale | Rust / MIT | reference + reuse | Fleet ledger, `execpolicy`, route-resolution, spillover, snapshots, `apply_patch` |
| Ruflo | TS / MIT | patterns | hierarchical sub-agents, memory-first coordination, ReasoningBank, HNSW, federation |
| codebase-memory-mcp | C / MIT | bundled sidecar | code knowledge graph (MCP) + 3D graph UI (`:9749`) |
| Osaurus | Swift / MIT | sidecar | local MLX inference + embeddings |
| OpenCode Zen | hosted | primary gateway | DeepSeek Pro/Flash, Qwen, GLM, Kimi, free models |
| dogfeed / ultrawhale-dogfood | TS / MIT · dataset Apache-2.0 | flywheel sink | HF dataset schema + exporter design |
| kompress-v8 | HF model / Apache-2.0 | auto-compaction | our own ONNX token-pruning model, run in-process (`ort`+`tokenizers`) |
| Honcho | Python / AGPL-3.0 | personalization sidecar | user-modeling representation + Dialectic API (Apple-container local) |
| Tailscale / ZeroTier | — | federation transport | tailnet identity/auth/crypto/NAT/discovery |
| Kitty graphics protocol + wgpu | — | rendering | shader-background compositing behind text |
| superpowers / caveman / BMAD | — | bundled skills | brainstorming/TDD/debugging · compressed output · agentic agile |
| Homebrew | — | plugin installer | confirmation-gated CLI provisioning |
| Lattimore, "Wild performance tricks" | — | dev bible | Rust performance practices (§9) |
