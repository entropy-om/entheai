---
id: concept
title: "Hybrid brain & fan-out"
group: Overview
order: 2
---

The core design of `entheai` bridges the **fluid entropy of prompt states** with the **rigid determinism of compiled checkpoints**.

## 🜂 The Genesis Thesis

> *"This program creates an automated quantum simulation playground where custom prompt states continuously morph a fluid field of infinite entropy back and forth into rigid, binary singularity checkpoints."*

- **Singularity is a Trap**: Hyper-optimized, fixed structure ends the generative phase.
- **Entropy is the True Infinite**: Raw, uncarved experiential soil where knowledge grows — especially from failure.
- **Structural Honesty (*AHOGY A DOLGOK VANNAK*)**: Never report better than reality, never hide worse than reality. When report and reality drift, reality wins.

## Tiered Decomposition & Execution

```text
User Request
    │
    ▼
Orchestrator (DeepSeek V4 Pro) ──▶ Decomposes task into role-tagged sub-tasks
    │
    ├─▶ Coder Sub-Agent (deepseek-v4-pro -> gemini-3.1-pro)  [Worktree A]
    ├─▶ Coder Sub-Agent (deepseek-v4-pro)                    [Worktree B]
    └─▶ Reviewer Sub-Agent (deepseek-v4-pro)                 [Worktree C]
            │
            ▼
   Empirical Verification Gate (`./scripts/check.sh`)
            │
            ▼
   Deterministic SHA-256 MergeSeal Integrated to Main
```

Sub-agents work concurrently in isolated git worktrees (`.worktrees/`). Unverifiable or failing worktree branches are rejected, and tracebacks feed back into memory to re-weight frozen node priors.
