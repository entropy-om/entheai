---
id: who
title: "Who it's for"
group: Overview
order: 3
---

`entheai` is built for solo developers and engineers on Apple Silicon Macs who require a personal, terminal-native hybrid coding agent that:

- **Respects Local & Keyless Compute**: Runs out of the box with a single key (`DEEPSEEK_API_KEY`, DeepSeek V4 by default), and keyless on request — the community node (`coder.vaked.dev`, `--model vaked/qwen3-coder:30b`; fan-out degrades to it automatically) or local Osaurus inference without sending tokens to cloud APIs.
- **Fans Out Complex Tasks**: Decomposes large refactors into model-matched sub-agent tasks running in parallel inside isolated git worktrees.
- **Enforces Absolute Rigor**: Rejects unverified self-reported sub-agent success through mandatory empirical test gates (`verify_required = true`) and deterministic SHA-256 `MergeSeal` signatures.
- **Compounds Knowledge**: Retains raw session experiences, reranks past context via native 1-bit LLM meshes, and continuously re-weights frozen node priors based on execution outcomes.
- **Operates with Structural Honesty**: Adheres to *AHOGY A DOLGOK VANNAK* — reporting reality as it is, naming both limits and remedies whenever errors occur.
