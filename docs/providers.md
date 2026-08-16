# Providers

entheai talks to any OpenAI-compatible API. Configure providers in `entheai.toml`, reference them as `<provider>/<model>`. Full key reference: [configuration.md](configuration.md).

## Built-in providers

entheai-config injects `deepseek`, `gemini`, `openrouter` and the keyless `vaked` block into every parsed config (a user-declared block with the same name is never overridden). A provider is *available* when it is declared in `[providers]` and its `api_key_env` (if any) is set in the environment.

### DeepSeek direct (recommended primary)

DeepSeek's own API. Two V4 models, both 1M context: `deepseek-v4-flash` (fast/cheap; the interactive default) and `deepseek-v4-pro` (strong; the built-in fan-out orchestrator, coder and reviewer tier — set `[router].orchestrator = "deepseek/deepseek-v4-pro"` when you also set a `default_model`, since a configured `default_model` outranks the built-in tier). The legacy `deepseek-chat` / `deepseek-reasoner` (V3 / R1) ids were discontinued on 2026-07-24.

```toml
[providers.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key_env = "DEEPSEEK_API_KEY"

default_model = "deepseek/deepseek-v4-flash"

[router]
orchestrator = "deepseek/deepseek-v4-pro"
```

### Gemini (first fallback)

Google's Gemini API through adk-rust's native client (`kind = "gemini"`). Used as the first fallback in the recommended role chains: `gemini/gemini-3.6-flash` for the flash tier, `gemini/gemini-3.1-pro-preview` for the pro tier. Note: `gemini-2.5-*` is no longer available to new API users (404), and Gemini 3.x over the OpenAI-compatible endpoint (`kind` unset) fails on the second turn of any tool call (`thought_signature` missing, HTTP 400) — keep `kind = "gemini"`.

```toml
[providers.gemini]
api_key_env = "GEMINI_API_KEY"
kind = "gemini"
base_url = ""   # unused for kind = "gemini"
```

### OpenRouter (second fallback)

Multi-provider gateway with hundreds of models. Configured as the third entry in the recommended chains (`openrouter/deepseek/deepseek-v4-flash`, `openrouter/deepseek/deepseek-v4-pro`).

```toml
[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
```

### vaked (free tier, keyless)

The public coder.vaked.dev node serving Qwen3-Coder-30B. No API key. On the fan-out path a role whose whole chain is unavailable degrades to `vaked/qwen3-coder:30b`; for keyless interactive use pass `--model vaked/qwen3-coder:30b`.

```toml
[providers.vaked]
base_url = "https://coder.vaked.dev/v1"
# No api_key_env — keyless
```

### Osaurus (local)

Local MLX inference server. No API key, no network, no cost. Best for latency-sensitive coding and embeddings.

```toml
[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"
# No api_key_env — local only
```

Requires [Osaurus](https://github.com/peterlodri-sec/Osaurus) running locally. Supports any model loadable by MLX (Qwen, Llama, DeepSeek Coder, etc.).

### OpenCode Zen (optional)

Cloud gateway with DeepSeek V4 Pro/Flash, Qwen 3.7, GLM, Kimi, and free models behind one API key. No longer the recommended primary; declare it if you want it.

```toml
[providers.zen]
base_url = "https://opencode.ai/zen/v1"
api_key_env = "OPENCODE_API_KEY"
```

### HuggingFace

```toml
[providers.huggingface]
base_url = "https://api-inference.huggingface.co/v1"
api_key_env = "HF_TOKEN"
```

## Custom providers

Any OpenAI-compatible endpoint works. Add a `[providers.<name>]` block:

```toml
[providers.my-custom]
base_url = "https://my-llm.internal/v1"
api_key_env = "MY_API_KEY"
```

Then use as `my-custom/<model>`.

## Model IDs

Format: `<provider>/<model>`. Split on the first `/`.

```
deepseek/deepseek-v4-flash
deepseek/deepseek-v4-pro
gemini/gemini-3.6-flash
gemini/gemini-3.1-pro-preview
openrouter/deepseek/deepseek-v4-flash
openrouter/deepseek/deepseek-v4-pro
vaked/qwen3-coder:30b
osaurus/qwen3-coder
zen/deepseek-v4-pro
huggingface/mistralai/Mistral-7B-Instruct-v0.3
```

## Role fallback chains

`[agents.<role>].model` (roles: `explore`, `coder`, `reviewer`, `test`, `docs`) is a preference-ordered fallback chain: the first entry whose provider is available wins. If none is, the role falls back to the orchestrator chain (`[router].orchestrator` -> `default_model` -> built-in chain: `deepseek-v4-pro` -> `gemini-3.1-pro-preview` -> OpenRouter for coder/reviewer, `deepseek-v4-flash` -> `gemini-3.6-flash` -> OpenRouter for explore/test/docs). On the fan-out path only, a still-unavailable provider degrades to `vaked/qwen3-coder:30b` (logged as a warning); interactive runs error loudly instead.

```toml
[agents.explore]
model = ["deepseek/deepseek-v4-flash", "gemini/gemini-3.6-flash", "openrouter/deepseek/deepseek-v4-flash"]

[agents.coder]
model = ["deepseek/deepseek-v4-pro", "gemini/gemini-3.1-pro-preview", "openrouter/deepseek/deepseek-v4-pro"]
```

## Embeddings

The memory crate uses an OpenAI-compatible `/v1/embeddings` endpoint on a configured provider. Unset `embed_provider` means no embeddings (memory stays offline-safe).

```toml
[memory]
embed_provider = "osaurus"        # a [providers.<name>] key
embed_model = "nomic-embed-text"
```

Any provider with an embeddings endpoint works — point `embed_provider` at another `[providers.<name>]` block if Osaurus isn't available.

## Availability

entheai does not fetch `/v1/models` or validate model ids against a catalog. A provider is available when it is declared in `[providers]` and its `api_key_env` is set; the model id is passed through to the endpoint as-is. Discontinued ids fail at request time, so keep chains on current models.
