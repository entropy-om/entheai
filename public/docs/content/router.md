---
id: router
title: "The tiered router"
group: Concepts
order: 2
---

The tiered router (`crates/router`) resolves task requirements to the optimal model tier based on role, cost, and complexity.

```text
                  ┌───────────────────────────────┐
                  │ Orchestrator (DeepSeek V4 Pro) │
                  └───────────────┬───────────────┘
                                  │ (Decompose & Plan)
         ┌────────────────────────┼────────────────────────┐
         ▼                        ▼                        ▼
┌─────────────────┐      ┌─────────────────┐      ┌─────────────────┐
│ Coder Sub-Agent │      │ Reviewer Agent  │      │  Explore Agent  │
│  (vaked/coder / │      │  (deepseek-r1 / │      │ (local osaurus) │
│   qwen3-coder)  │      │  v4-pro review) │      │                 │
└─────────────────┘      └─────────────────┘      └─────────────────┘
```

## Role Mapping Configuration

Define role preferences in `entheai.toml`:

```toml
[router]
orchestrator = "zen/deepseek-v4-pro"   # Strong reasoning model for planning
max_parallel = 4                         # Concurrent sub-agent worktrees
max_turns = 200                          # Per-task turn cap

[agents.explore]
model = ["osaurus/qwen3-coder"]

[agents.coder]
model = ["vaked/coder", "deepseek/deepseek-chat"]

[agents.reviewer]
model = ["deepseek/deepseek-reasoner"]

[agents.test]
model = ["vaked/coder"]

[agents.docs]
model = ["vaked/coder"]
```

When a role is unconfigured, it falls back to `[router].orchestrator`, then `default_model`.
