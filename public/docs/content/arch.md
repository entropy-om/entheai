---
id: arch
title: "Crate map & system"
group: Architecture
order: 1
badgeText: Architecture
---

entheai is a Rust workspace. `core` is built on [adk-rust](https://github.com/zavora-ai/adk-rust); the orchestrator fans out to model-matched sub-agents in isolated git worktrees.

```text
entheai-core        · EntheaiAgent (adk-rust), tool dispatch, memory-aware runs
entheai-router      · role → model resolution, agent factory
entheai-orchestrator· fan-out decomposition, worktree pool, merge/verify
entheai-memory      · 5-namespace SQLite + vector store
entheai-memory-pp   · prompt-processing, frozen nodes, BrainJudge
entheai-tui         · streaming chat, brain-ring + swarm visualization
```
