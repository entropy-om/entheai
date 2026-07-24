---
id: providers
title: Providers
group: Configuration
order: 1
badgeText: Configuration
---

`entheai` communicates with any OpenAI-compatible API backend via `adk-rust`. Specify provider configurations under `[providers.<name>]` in `entheai.toml`.

| Provider | Type | API Key Requirement | Description |
|---|---|---|---|
| `vaked` | Community Node | Keyless (No key needed) | Free community inference endpoint on `coder.vaked.dev`. |
| `osaurus` | Local | Keyless | Local models running on Apple Silicon via Osaurus (`127.0.0.1:1337`). |
| `zen` | Cloud | `OPENCODE_API_KEY` | OpenCode Zen gateway (DeepSeek V4 Pro, Qwen, etc.). |
| `deepseek` | Cloud | `DEEPSEEK_API_KEY` | DeepSeek direct API (V3, R1). |
| `openrouter` | Aggregator | `OPENROUTER_API_KEY` | OpenRouter model gateway. |
| `hf` | Router | `HUGGINGFACE_API_KEY` | Hugging Face Serverless Inference API. |

## Error Resolution Policy

Per `entheai`'s structural honesty doctrine, every runtime provider error explicitly names both the limit **and** the remedy:

- **Missing API Key**:
  - *Limit*: `Error: env var "OPENCODE_API_KEY" not set for provider "zen"`
  - *Remedy*: Set `OPENCODE_API_KEY` in `.env` or switch to the keyless `vaked` / `osaurus` provider via `--model vaked/coder`.
- **Unreachable Endpoint**:
  - *Limit*: `Error: building client for provider "osaurus": failed to connect to 127.0.0.1:1337`
  - *Remedy*: Launch Osaurus locally (`osaurus serve`) or update `base_url` in `entheai.toml`.
