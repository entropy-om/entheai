---
name: deepseek-harness
description: Integration with DeepSeek Harness (dsh) — the plugin-first agent runtime, Cordis spatiotemporal architecture, and trajectory inspection engine.
---

# DeepSeek Harness (`dsh`) Skill

DeepSeek Harness (`deepseek-ai/deepseek-harness`) is DeepSeek's open-source agent runtime framework designed around the philosophy that **"Everything is a Plugin"** using the **Cordis** meta-framework.

---

## 1. Core Architecture

- **Cordis Meta-Framework**: Everything (models, tools, memory, sandboxes, scheduling, UI) is an extensible plugin.
- **Append-Only Trajectory Event Log**: Records prompts, reasoning steps, tool calls, and sub-agent forks for exact replay and branching.
- **Local-First & Privacy**: Stores session trajectories, logs, and artifacts locally.
- **Web Workspace**: Interactive visualization and trajectory debugger.

---

## 2. Running DeepSeek Harness with Entheai

### A. Launch the Interactive Web Workspace
To start the local DeepSeek Harness web interface:
```bash
# One-line npx runner (runs at http://127.0.0.1:3080)
npx -y @deepseek-ai/dsh web
```

### B. Use via MCP inside Entheai
DeepSeek Harness is mounted as an MCP server in `entheai.toml`:
```toml
[mcp.deepseek-harness]
command = "npx"
args = ["-y", "@deepseek-ai/dsh", "mcp"]
```
All DSH tools are exposed as `deepseek-harness__<tool>` to Entheai's agent loop.

### C. CLI Direct Execution
```bash
# Run a prompt through DSH standard profile
npx -y @deepseek-ai/dsh run "Analyze the git repository state"
```

---

## 3. Synergy with the Vaked Constellation

| Feature | DeepSeek Harness (`dsh`) | Entheai Ecosystem |
|---|---|---|
| **Primary Model** | DeepSeek V4 Flash / Pro | DeepSeek V4 Flash/Pro (direct API) + Gemini fallback |
| **Tool Layer** | Cordis Plugins | Native Rust Tools + `smart-tree` (`st`) + `magiscanner` |
| **Memory** | Local Event Log | 5-Namespace Vector Memory (MEM|8 4-Band Resonance) |
| **Orchestration** | Spatiotemporal Composability | Parallel Git Worktree Fan-Out + NATS JetStream Swarm |
| **Interface** | DSH Web UI (:3080) | Entheai TUI (Ratatui) + Floating Companion Beacon (180x180) |

---

## 4. Best Practices

1. **Debugging Complex Trajectories**: When an autonomous fan-out run produces non-obvious reasoning branches, use `npx @deepseek-ai/dsh web` to inspect the exact decision graph.
2. **Hybrid Workflows**: Use Entheai's native Rust execution on Apple Silicon Unified Memory for high-throughput code editing, and invoke DSH plugins for modular sandbox isolation.
