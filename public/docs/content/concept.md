---
id: concept
title: "Hybrid brain & fan-out"
group: Overview
order: 2
---

The **tiered hybrid brain** separates planning from execution. A capable cloud model reasons about the whole task; cheaper local or specialized models do the parallel work.

## Fan-out

The orchestrator decomposes a task into units and dispatches a sub‑agent per unit — each in its own git worktree, each on the model that best fits its role. Work merges back only after that unit's tests pass.

```text
task ──▶ orchestrator (deepseek/v4-pro)
           ├─ coder    · osaurus/qwen2.5-coder
           ├─ test     · deepseek/v4-pro
           └─ review   · osaurus/deepseek-r1
                         ▼
              merge + verify ▶ main
```
