# entheai-worker — Headless Remote Agent Executor

**Status:** spec — **partially implemented in v0.2.1 (F2.1); see "Implemented today" below.** · **Vision version:** v0.4+ · **Author:** entheai

## Summary

`entheai-worker` is the portable, headless companion binary to the main `entheai` TUI. It runs the full agent loop (`core`) plus tools, memory, and comms on any-OS peers connected over a Tailscale tailnet. The orchestrator dispatches sub-agents to workers for remote execution; the worker returns results (patches, outputs, trajectories) back to the orchestrator for join/reduce.

**Think of it as:** the agent runtime without the face. Same agent loop, same tools, same memory engine — just no TUI, no viz, no Kitty shaders, no macOS-specific sandbox. Runs on Linux, macOS (Intel + Apple Silicon), and Windows (best-effort).

## Implemented today (v0.2.1 — F2.1)

The first working slice ships a **NATS JetStream** transport — *not* the gRPC/`comms` design sketched below, which remains the forward vision. Plan: [`docs/superpowers/plans/2026-07-20-federation-f2-distributed-swarm.md`](superpowers/plans/2026-07-20-federation-f2-distributed-swarm.md); design: [federation spec §4](superpowers/specs/2026-07-20-entheai-nats-federation-design.md).

- **`entheai-worker --serve`** — connects to the `[nats]` hub, pulls `WorkItem`s off a durable **JetStream WorkQueue** (`entheai.work.coder`; exactly-one delivery + `ack_wait`/`max_deliver` leases), materializes the repo from a **git bundle** in the JetStream **object store**, runs the coder (`run_coder_once`) in an isolated clone, and bundles the delta back. `--test-coder '<shell>'` swaps the LLM step for a deterministic command (used by the E2E test).
- **`entheai-worker --dispatch --role coder --task "…"`** — bundles the current repo, enqueues a `WorkItem`, awaits the `WorkResult` over core NATS, and applies the returned delta to a `fed/…` branch.
- **`entheai-worker --role <r> --task <t> --worktree <path>`** — the original one-shot local mode (`run_coder_once` against a given worktree), unchanged.
- Opt-in via `[federation]` (reuses `[nats]` creds). **Fail-safe** — no hub, no problem; the caller runs locally.
- **Shipped (F2.3):** worker sandbox hardening — Linux Landlock + seccomp + drop-root (production, jail-proven) and macOS `sandbox_init` (best-effort). The `comms`/gRPC + `learning`/`dogfeed` pushback described below is still aspirational. Even confined, a worker runs model output with full tools + open network (and inherits provider/NATS env keys) — run `--serve` **only on trusted, tailnet-only nodes**.

The sections below are the broader **v0.3 design vision**; the transport and several crates named there (`comms`, `learning`, `dogfeed`, `health`, `session`, `plugins`) are aspirational or realized differently by F2.1.

## Architecture

```
┌─────────────────────────────────────────────┐
│  entheai (macOS TUI)                         │
│   orchestrator → fan-out plan                │
│   executor seam: Local | Remote              │
│   comms → Tailscale dispatch                 │
└──────────────────┬──────────────────────────┘
                   │ Tailscale tailnet
    ┌──────────────┼──────────────┐
    ▼              ▼              ▼
┌────────┐  ┌──────────┐  ┌──────────┐
│ worker │  │  worker  │  │  worker  │
│ macOS  │  │  Linux   │  │  Linux   │
│ (idle) │  │ (coder)  │  │ (test)   │
└────────┘  └──────────┘  └──────────┘
```

### Crate map

Worker compiles a subset of the workspace (`--no-default-features`, macOS-only crates gated):

| Crate | Included? | Why |
|---|---|---|
| `core` | ✅ | Agent loop, orchestration, event bus |
| `providers` | ✅ | OpenAI-compatible client (Zen, Osaurus, etc.) |
| `router` | ✅ | Model selection per dispatched role |
| `agents` | ✅ | Sub-agent execution, worktree isolation |
| `memory` | ✅ | Local SQLite + vector store |
| `learning` | ✅ | Trajectory capture (fed back to orchestrator) |
| `dogfeed` | ✅ | Export trajectories to HF dataset |
| `mcp` | ✅ | MCP client (codebase-memory-mcp optional) |
| `skills` | ✅ | Skill discovery + invocation |
| `plugins` | ✅ | Managed CLI provisioning (per-platform) |
| `tools` | ✅ | fs/shell/search/apply_patch |
| `comms` | ✅ | Tailscale transport (peer discovery, dispatch) |
| `session` | ✅ | SQLite persistence, crash-resumable ledger |
| `health` | ✅ | Panic hook + liveness reporter → Sonar |
| `config` | ✅ | TOML config (providers, roles, tailnet nodes) |
| `tui` | ❌ | macOS-only; worker is headless |
| `viz` | ❌ | macOS-only; no GPU rendering |
| `sandbox` | ✅ | Coder confinement: Landlock/seccomp (Linux, lead — F2.3/A3) · Seatbelt (macOS) |
| `permission` | ✅ | Policy gate (YOLO/allowlist per dispatched role) |

### Binary surface

```
entheai-worker [OPTIONS]

OPTIONS:
  --config <PATH>        Path to worker.toml (default: ./entheai.toml)
  --tailnet <NAME>       Tailscale tailnet to join
  --name <NAME>          Human-readable node name (reported to orchestrator)
  --capabilities <LIST>  Comma-separated roles this worker accepts
                         (e.g. "coder,test,merge")
```

The worker **does not** accept a prompt on the CLI. It listens for dispatched sub-agents over the tailnet. When idle, it reports its capabilities and health to the orchestrator via the `comms` heartbeat.

## Lifecycle

### 1. Startup
1. Parse config (`worker.toml` or `entheai.toml`)
2. Open SQLite session ledger (`session.db`)
3. Register with the tailnet (Tailscale LocalAPI or `tsnet` embedded)
4. Announce capabilities + health to orchestrator
5. Block on the dispatch channel

### 2. Dispatch (remote execution)
1. Orchestrator selects worker by role match + node availability
2. Sends a `Dispatch` message over the tailnet (gRPC or msgpack over TLS):
   ```json
   {
     "task_id": "uuid",
     "role": "coder",
     "prompt": "Implement the User struct in src/models/user.rs",
     "model": "osaurus/qwen3-coder",
     "worktree_base": "abc123...",
     "skills": ["superpowers/test-driven-development"],
     "tools": ["fs", "shell", "search"]
   }
   ```
3. Worker creates a git worktree (or reuses a pool)
4. Runs the agent loop (`core::Agent::run_task`) with the dispatched prompt
5. Publishes every step to the event bus (captured by `learning` + `dogfeed`)
6. Returns results:
   ```json
   {
     "task_id": "uuid",
     "status": "ok" | "error",
     "patch": "...",
     "trajectory": [...],
     "build_output": "...",
     "test_output": "..."
   }
   ```

### 3. Shutdown
- Graceful: orchestrator sends `Shutdown` → worker drains running tasks, closes DB, exits
- Crash: Sonar detects liveness loss, notifies orchestrator; session ledger allows resume

## Sandboxing

Each dispatched coder runs inside a dedicated **`--sandbox-run` child** that the
`--serve` parent spawns per task (see `crates/sandbox`). Confinement is applied in
the child while it is still single-threaded, before its async runtime starts, and
is irreversible for that process. The posture is set by `[federation] sandbox`
(default `permissive`) — there is **no** `[worker] sandbox` knob:

| Mode | Behavior |
|---|---|
| `strict` | Refuse to run the coder if confinement can't be applied on this host (child exits `3`; the worker reports the item failed). |
| `permissive` (default) | Attempt confinement; if unavailable, warn and run the coder unconfined. |
| `off` | Never attempt confinement (the pre-F2.3 behavior). |

**Platform backends:**

| Platform | Strategy |
|---|---|
| Linux | Landlock filesystem jail + seccomp syscall denylist + drop-root — the **production backend**. `availability()` reports *available* on any Landlock-capable kernel; jail-proven by a forked self-test (`cargo test -p entheai-sandbox -- --ignored`) that verifies an out-of-worktree read is denied by Landlock and `unshare(2)` is blocked by seccomp. |
| macOS | Best-effort `sandbox_init` (Seatbelt) profile. For local `--serve` testing, not production. |
| other | No confinement backend — `availability()` reports unavailable. |

**Filesystem confinement** differs by backend — the strength is not the same:

- **Linux (production backend):** default-**deny** via Landlock. The coder's worktree
  is the single read-write directory; every other path is denied except a read-only
  allow-list — the toolchain (`/usr`, `/lib*`, `/bin`, plus `~/.cargo` and `~/.rustup`
  when a `[fanout] verify` command is configured), CA certs (`/etc/ssl`,
  `/etc/ca-certificates`), `/etc/resolv.conf`, `/etc/hosts`, and `/tmp`.
- **macOS (shipping today, best-effort):** the `sandbox_init` profile is `allow
  default` with targeted **denies** — it confines **writes** to the worktree (plus
  `/tmp` and the macOS temp dirs) and denies **reads** of a few common secret dirs
  (`~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.config/gcloud`), but **all other reads stay
  open**. It blocks worktree-escape writes and the obvious credential reads; it is
  deliberately *not* the Linux default-deny model. For local testing, not production.

**Privilege drop.** If the worker is running as root, the confined coder drops to the
invoking user (`SUDO_UID`/`SUDO_GID`) or to `nobody` (65534) — untrusted model output
never keeps root. Off-root this is a no-op.

**Network stays open (documented residual risk).** The coder needs outbound access to
reach the LLM endpoint, so the sandbox does **not** restrict the network. A confined
coder can still open arbitrary outbound connections; run `--serve` only on trusted,
tailnet-only nodes.

**Env-borne secrets are exposed to the coder (residual risk).** The `--sandbox-run`
child inherits the worker's environment — including the provider API key(s) and NATS
credentials loaded from `.env`. Filesystem confinement does not cover process env, and
the network is open, so a malicious coder can read its own environment and exfiltrate
those keys. The `~/.ssh`/`~/.aws` read-denies protect *host* secrets, not the app's own
in-process credentials — which is the core reason `--serve` must run only on trusted,
tailnet-only nodes. Env-scrubbing the child and an egress allow-list are future
hardening (F2.4+).

**Startup posture.** At `--serve` startup the worker logs one line —
`worker serving · sandbox=<mode> · confinement=<available | unavailable: <reason>>`.
Under `strict` on a host where confinement is unavailable, that line is a prominent
warning that real coders will refuse to run there.

## Concurrency & the shared base

A `--serve` worker runs up to `[federation] max_concurrent_coders` coders at once
(default 4). Coders spend almost all their time waiting on the model, so running several
together multiplies throughput at little CPU cost. A bounded semaphore caps how many run
at once; a permit is taken before each claim, so an item is never claimed unless there's
capacity to process it.

To keep that concurrency from multiplying memory, all coders on a given base commit share
**one** materialized copy of it. The worker keeps a small per-node cache of bare repos
(one per base commit); each coder attaches a cheap **detached git worktree** off the
shared repo, so the object store is shared rather than copied. A coder's changes live in
its own worktree and are bundled back as usual.

It's a pure optimization. The cache lookup + worktree attach run under a short deadline,
and on any failure the worker falls back to a full clone for that coder. Each result
carries a `base = hit | miss | degraded:<reason>` tag so the dispatcher can see when the
fast path was skipped and why. A base that a live coder is still attached to is never
evicted from the cache.

## Memory & learning

The worker runs its own `memory` crate instance (local SQLite + Osaurus embeddings). After task completion:
- `learnings` and `trajectories` are **pushed back** to the orchestrator
- `tools` namespace is ephemeral (cleared per task)
- `subagents` namespace is shared with the orchestrator over the tailnet bus

The orchestrator merges worker trajectories into its own ReasoningBank, improving future router decisions.

## Configuration

```toml
# worker.toml (subset of entheai.toml; shared [providers] section)

[worker]
name = "studio-m3-max"
capabilities = ["coder", "test", "merge"]
# Coder confinement is NOT set here — it lives under [federation] sandbox
# ("strict" | "permissive" | "off"); see the Sandboxing section above.

[providers]
# Same as main entheai.toml; worker uses local Osaurus or cloud providers
zen.api_key_env = "OPENCODE_API_KEY"

[router]
coder = ["osaurus/qwen3-coder", "zen/deepseek-v4-flash"]

[comms]
tailnet = "peterlodri-sec.tailnet.ts.net"
heartbeat_interval_s = 5

[memory]
db_path = "/var/lib/entheai/worker.db"
embedding_url = "http://127.0.0.1:1337/v1"
```

## Open questions

- **`tsnet` embed vs. LocalAPI**: No official Rust `tsnet` crate exists. Options: (a) use Tailscale LocalAPI (`tailscale status --json` + `/localapi/v0/...`), (b) shell out to `tailscale` CLI, (c) wait for an official Rust SDK. v0.3 ships with LocalAPI.
- **Worker worktree pools**: Pre-warmed git worktrees reduce dispatch latency. How many? LRU eviction? v0.3 starts with on-demand creation.
- **Cross-platform sandboxing**: Linux `landlock` is clean but kernel 5.13+. `bwrap` is more portable but requires user namespaces. Windows job objects are limited. Document the gap clearly.
- **Auth between orchestrator and worker**: Tailscale provides identity + encrypted transport. Is that sufficient, or do we need an application-level token?
