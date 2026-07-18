# entheai-worker — Headless Remote Agent Executor

**Status:** spec · **Version:** v0.3 · **Author:** entheai

## Summary

`entheai-worker` is the portable, headless companion binary to the main `entheai` TUI. It runs the full agent loop (`core`) plus tools, memory, and comms on any-OS peers connected over a Tailscale tailnet. The orchestrator dispatches sub-agents to workers for remote execution; the worker returns results (patches, outputs, trajectories) back to the orchestrator for join/reduce.

**Think of it as:** the agent runtime without the face. Same agent loop, same tools, same memory engine — just no TUI, no viz, no Kitty shaders, no macOS-specific sandbox. Runs on Linux, macOS (Intel + Apple Silicon), and Windows (best-effort).

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
| `sandbox` | ❌ | macOS Seatbelt; worker uses best-effort sandboxing |
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

| Platform | Strategy |
|---|---|
| macOS | Seatbelt profile (same as `entheai`) |
| Linux | `bwrap` (bubblewrap) / `landlock` / `seccomp` — best-effort |
| Windows | Job objects + restricted token — best-effort |

Worker sandboxing is **best-effort** on non-macOS. The orchestrator can set `sandbox: strict` (refuse dispatch if full sandboxing unavailable) or `sandbox: permissive` (accept reduced isolation).

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
sandbox = "strict"            # strict | permissive | off

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
