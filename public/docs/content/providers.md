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
| `deepseek` | Cloud (default) | `DEEPSEEK_API_KEY` | DeepSeek direct API (`api.deepseek.com/v1`): `deepseek-v4-flash` (fast/cheap) and `deepseek-v4-pro` (strong), both 1M context. |
| `gemini` | Cloud (fallback) | `GEMINI_API_KEY` | Google Gemini via the native API (`kind = "gemini"`): `gemini-3.6-flash`, `gemini-3.1-pro-preview`. |
| `openrouter` | Aggregator (fallback) | `OPENROUTER_API_KEY` | OpenRouter model gateway, e.g. `openrouter/deepseek/deepseek-v4-pro`. |
| `vaked` | Community Node | Keyless (No key needed) | Free community inference endpoint on `coder.vaked.dev` (`vaked/qwen3-coder:30b`); the fan-out's last-resort fallback. |
| `osaurus` | Local | Keyless | Local models running on Apple Silicon via Osaurus (`127.0.0.1:1337`). |
| `zen` | Cloud (optional) | `OPENCODE_API_KEY` | OpenCode Zen gateway (DeepSeek V4 Pro, Qwen, etc.). |
| `hf` | Router | `HUGGINGFACE_API_KEY` | Hugging Face Serverless Inference API. |

`deepseek`, `gemini`, `openrouter` and `vaked` are built in: they are injected into every parsed config (a user block with the same name always wins), so they work without a `[providers.*]` entry once their key is set.

## Error Resolution Policy

Per `entheai`'s structural honesty doctrine, every runtime provider error explicitly names both the limit **and** the remedy:

- **Missing API Key**:
  - *Limit*: `Error: env var "DEEPSEEK_API_KEY" not set for provider "deepseek"`
  - *Remedy*: Set `DEEPSEEK_API_KEY` in `.env` or switch to the keyless `vaked` / `osaurus` provider via `--model vaked/qwen3-coder:30b`.
- **Unreachable Endpoint**:
  - *Limit*: `Error: building client for provider "osaurus": failed to connect to 127.0.0.1:1337`
  - *Remedy*: Launch Osaurus locally (`osaurus serve`) or update `base_url` in `entheai.toml`.
