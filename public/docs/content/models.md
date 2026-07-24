---
id: models
title: "Models & ids"
group: Configuration
order: 2
---

Every model spec in `entheai` follows the `<provider>/<model>` convention:

```toml
# Examples of valid model specifications
vaked/coder
osaurus/qwen3-coder
zen/deepseek-v4-pro
deepseek/deepseek-reasoner
openrouter/anthropic/claude-3.5-sonnet
```

## Resolution Mechanics

1. `entheai` splits the spec on the first slash `/`.
2. The left component looks up `[providers.<name>]` in `entheai.toml`.
3. The right component becomes the model string passed in OpenAI-compatible API requests.

> [!NOTE]
> When a model spec is passed via `--model "<provider>/<model>"`, it overrides `default_model` for that invocation without modifying `entheai.toml`.
