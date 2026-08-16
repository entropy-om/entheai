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
│ (DeepSeek V4 Pro│      │ (DeepSeek V4 Pro│      │(DeepSeek V4     │
│  -> Gemini Pro) │      │  -> Gemini Pro) │      │ Flash -> Gemini)│
└─────────────────┘      └─────────────────┘      └─────────────────┘
```

## Role Mapping Configuration

Define role preferences in `entheai.toml`:

```toml
[router]
orchestrator = "deepseek/deepseek-v4-pro"   # Strong reasoning model for planning
max_parallel = 4                            # Concurrent sub-agent worktrees
max_turns = 200                             # Per-task turn cap

[agents.explore]
model = ["deepseek/deepseek-v4-flash", "gemini/gemini-3.6-flash", "openrouter/deepseek/deepseek-v4-flash"]

[agents.coder]
model = ["deepseek/deepseek-v4-pro", "gemini/gemini-3.1-pro-preview", "openrouter/deepseek/deepseek-v4-pro"]

[agents.reviewer]
model = ["deepseek/deepseek-v4-pro", "gemini/gemini-3.1-pro-preview", "openrouter/deepseek/deepseek-v4-pro"]

[agents.test]
model = ["deepseek/deepseek-v4-flash", "gemini/gemini-3.6-flash", "openrouter/deepseek/deepseek-v4-flash"]

[agents.docs]
model = ["deepseek/deepseek-v4-flash", "gemini/gemini-3.6-flash", "openrouter/deepseek/deepseek-v4-flash"]
```

Each `[agents.<role>].model` list is a preference-ordered fallback chain: the router walks it and picks the first entry whose provider is available (declared in `[providers]` and, if it needs one, its `api_key_env` set). If no entry is available, the role falls back to the orchestrator chain (`[router].orchestrator`, then `default_model`), and finally to the built-in chain: `deepseek/deepseek-v4-pro` -> `gemini/gemini-3.1-pro-preview` -> `openrouter/deepseek/deepseek-v4-pro` for `coder` and `reviewer`, `deepseek/deepseek-v4-flash` -> `gemini/gemini-3.6-flash` -> `openrouter/deepseek/deepseek-v4-flash` for `explore`, `test` and `docs`. On the fan-out path only, a still-unavailable provider degrades to the keyless free tier `vaked/qwen3-coder:30b` (with a warning); interactive runs error loudly instead.
