# Architecture

entheai is a macOS-native hybrid coding agent CLI built as a Rust workspace. One orchestrator, many sub-agents, all backed by persistent memory and a self-learning feedback loop.

## High-level flow

```
User prompt (TUI)
  → orchestrator plans + assigns effort scores
    → router picks best model per sub-agent
      → fan-out: multiple coders run in isolated git worktrees
        → join/reduce: merge patches, build, test
          → store learnings + trajectory
            → dogfeed exporter (opt-in)
```

## Workspace crate map

```
Cargo.toml                          # workspace root (resolver=2)
├── bin/entheai/                    # CLI binary (macOS, full TUI)
├── bin/entheai-worker/             # headless agent executor (any OS, v0.3)
├── crates/config/                  # TOML deserialization
├── crates/providers/               # OpenAI-compatible client + registry
├── crates/core/                    # Agent loop + event bus
├── crates/router/                  # Model selection per role/task
├── crates/orchestrator/            # Fan-out planning + DAG execution
├── crates/tools/                   # Built-in tools (fs, shell, search)
├── crates/permission/              # YOLO / allowlist / ask gate
├── crates/tui/                     # ratatui terminal UI
├── crates/memory/                  # 5-namespace SQLite + vector store
├── crates/radio/                   # In-TUI music (yt-dlp + rodio)
└── crates/companion/               # QR-code session beacon
```

Crate names use dashes (`entheai-core`); Rust modules use underscores (`entheai_core`).

## Tiered hybrid brain

| Tier | Provider | Use |
|---|---|---|
| Cloud orchestrator | DeepSeek V4 Pro (via OpenCode Zen) | Planning, decomposition, complex reasoning |
| Cheap cloud workers | DeepSeek V4 Flash, Qwen 3.7, GLM, Kimi | Implementation, testing, docs |
| Local | Osaurus (MLX, `127.0.0.1:1337`) | Low-latency coding, embeddings, privacy |

## The event bus

Every agent step (thinking, tool call, tool result, build output) is published to an internal event bus. Two consumers tap it:

1. **Learning** — captures trajectories, scores outcomes, tunes the router
2. **Dogfeed** — exports trajectories to a Hugging Face dataset (`PeetPedro/ultrawhale-dogfood`, opt-in)

## Memory model

Two tiers, five namespaces, one `Memory` trait:

| Namespace | Tier | Stores |
|---|---|---|
| `codebase` | long-term | Symbols, call graph, ADRs (federated to MCP) |
| `learnings` | long-term | Durable facts, preferences, solutions |
| `trajectories` | long-term | Reasoning paths + outcomes + scores |
| `tools` | working | Tool results, large output spillover |
| `subagents` | working | Per-sub-agent scratch + outputs |

Storage: local SQLite (WAL, 256 MB mmap) + vector embeddings via Osaurus. Flat cosine search below ~5k vectors; auto-HNSW above.

## Fan-out execution

1. Orchestrator decomposes task into a DAG of `{role, prompt, model, deps}` nodes
2. Router picks `(provider, model)` per node based on role, budget, and learned win-rates
3. Each `coder` node gets its own git worktree (isolated writes)
4. Join/reduce: merge patches, resolve conflicts, build + test
5. On success: integrate. On failure: diff for approval (unless `--yolo`)

## Extension points

Four composable ways to extend entheai:

| Mechanism | Format | Example |
|---|---|---|
| **Native tools** | Rust `Tool` trait | `read_file`, `run_shell`, `search` |
| **Skills** | Claude-Code `SKILL.md` packs | superpowers, caveman, BMAD |
| **Plugins** | Managed external CLIs | `hcloud`, `doctl`, `aws` (Homebrew provisioned) |
| **MCP servers** | MCP protocol (stdio/HTTP) | codebase-memory-mcp (bundled) |

## Platform

- **Main binary:** macOS 15.5+, Apple Silicon only. Uses Metal, Seatbelt, Kitty graphics protocol. Needs Ghostty / Kitty / WezTerm.
- **Worker:** Portable — any OS with Tailscale. Headless, no TUI/viz.
- **Sidecars:** Osaurus (MLX inference, `:1337`), Sonar (crash monitor), codebase-memory-mcp (code graph, `:9749`).

## Performance

- Release builds: `opt-level=3`, `lto="fat"`, `codegen-units=1`, `target-cpu=native`
- `mimalloc` global allocator on macOS
- `panic="unwind"` — sub-agent tokio-task panics must stay catchable
- Hot paths: agent loop, vector search, fan-out scheduler, Kitty render loop
