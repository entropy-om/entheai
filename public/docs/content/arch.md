---
id: arch
title: "Crate map & system"
group: Architecture
order: 1
badgeText: Architecture
---

entheai is a Rust workspace. The orchestrator drives providers, Osaurus, a codebase‑memory MCP server, and the sub‑agent pool.

```text
entheai-core      · agent loop, router
entheai-providers · osaurus, zen, deepseek
entheai-memory    · MCP server, 5 namespaces
entheai-tui       · shader + codebase graph
entheai-agents    · worktree pool, merge/verify
```
