---
id: models
title: "Models & ids"
group: Configuration
order: 2
---

Every model spec in `entheai` follows the `<provider>/<model>` convention:

```toml
# Examples of valid model specifications
deepseek/deepseek-v4-flash            # default: fast/cheap, 1M context
deepseek/deepseek-v4-pro              # strong tier: orchestrator, coder, reviewer
gemini/gemini-3.6-flash
gemini/gemini-3.1-pro-preview
openrouter/deepseek/deepseek-v4-pro
vaked/qwen3-coder:30b                 # keyless free tier
osaurus/qwen3-coder
```

DeepSeek V4 is the default: `deepseek/deepseek-v4-flash` when `--model` is omitted, `deepseek/deepseek-v4-pro` for the fan-out orchestrator. The legacy `deepseek-chat` / `deepseek-reasoner` ids were discontinued on 2026-07-24; use the V4 ids above.

## Resolution Mechanics

1. `entheai` splits the spec on the first slash `/`.
2. The left component looks up `[providers.<name>]` in `entheai.toml`.
3. The right component becomes the model string passed in OpenAI-compatible API requests.

> [!NOTE]
> When a model spec is passed via `--model "<provider>/<model>"`, it overrides `default_model` for that invocation without modifying `entheai.toml`.
