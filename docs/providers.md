# Providers

entheai talks to any OpenAI-compatible API. Configure providers in `entheai.toml`, reference them as `<provider>/<model>`.

## Built-in providers

### OpenCode Zen (recommended primary)

Cloud gateway with DeepSeek V4 Pro/Flash, Qwen 3.7, GLM, Kimi, and free models — one API key.

```toml
[providers.zen]
base_url = "https://opencode.ai/zen/v1"
api_key_env = "OPENCODE_API_KEY"

default_model = "zen/deepseek-v4-pro"
```

### Osaurus (local)

Local MLX inference server. No API key, no network, no cost. Best for latency-sensitive coding and embeddings.

```toml
[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"
# No api_key_env — local only
```

Requires [Osaurus](https://github.com/peterlodri-sec/Osaurus) running locally. Supports any model loadable by MLX (Qwen, Llama, DeepSeek Coder, etc.).

### DeepSeek (direct)

```toml
[providers.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key_env = "DEEPSEEK_API_KEY"
```

### OpenRouter

Multi-provider gateway with hundreds of models.

```toml
[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
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
zen/deepseek-v4-pro
zen/deepseek-v4-flash
zen/qwen3.7-coder
osaurus/qwen3-coder
deepseek/deepseek-chat
openrouter/anthropic/claude-sonnet-4
huggingface/mistralai/Mistral-7B-Instruct-v0.3
```

## Embeddings

The memory crate uses an OpenAI-compatible `/v1/embeddings` endpoint. Default: Osaurus.

```toml
[memory]
embedding_url = "http://127.0.0.1:1337/v1"
embedding_model = "nomic-embed-text"
```

Any provider with an embeddings endpoint works — point `embedding_url` at Zen, DeepSeek, or OpenRouter if Osaurus isn't available.

## Live model catalog

entheai fetches `/v1/models` at startup to discover available models. The router uses this to validate model preferences and detect deprecations. Never hardcode model names — the catalog is authoritative.
