# Configuration Reference

All configuration lives in `entheai.toml` at the project root. Every key is optional with sensible defaults.

## `[providers.<name>]`

```toml
[providers.zen]
base_url = "https://opencode.ai/zen/v1"
api_key_env = "OPENCODE_API_KEY"

[providers.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key_env = "DEEPSEEK_API_KEY"

[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"
# No api_key_env needed for local inference

[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
```

| Key | Required | Description |
|---|---|---|
| `base_url` | yes | OpenAI-compatible API base (include `/v1`) |
| `api_key_env` | no | Environment variable holding the API key |

## `default_model`

```toml
default_model = "zen/deepseek-v4-pro"
```

Format: `<provider>/<model>`. The provider name must match a `[providers.<name>]` key.

## `[router]`

```toml
[router]
orchestrator = "zen/deepseek-v4-pro"
fanout_threshold = 5
max_parallel = 8
default_coder = "zen/deepseek-v4-flash"
```

| Key | Default | Description |
|---|---|---|
| `orchestrator` | `default_model` | Model for planning and decomposition |
| `fanout_threshold` | 5 | Effort score threshold for fan-out vs inline |
| `max_parallel` | 8 | Max concurrent sub-agents |
| `default_coder` | `default_model` | Fallback model for coder sub-agents |

## `[agents.<role>]`

```toml
[agents.coder]
tools = ["fs", "shell", "search"]
effort = "high"
model = ["zen/deepseek-v4-flash", "osaurus/qwen3-coder"]
skills = ["superpowers/test-driven-development"]

[agents.test]
tools = ["shell"]
effort = "medium"
model = ["zen/deepseek-v4-flash"]
```

| Key | Description |
|---|---|
| `tools` | Allowed tools for this role |
| `effort` | `low` / `medium` / `high` — used by orchestrator |
| `model` | Ordered list of model preferences |
| `skills` | Skills auto-loaded for this role |

## `[companion]`

```toml
[companion]
enabled = true
always_on_top = true
```

Spawns a QR-code beacon window for phone pairing over Tailscale. Requires `entheai-companion` binary in PATH.

## `[memory]`

```toml
[memory]
db_path = "~/.cache/entheai/memory.db"
embedding_url = "http://127.0.0.1:1337/v1"
embedding_model = "nomic-embed-text"
```

| Key | Default | Description |
|---|---|---|
| `db_path` | `~/.cache/entheai/memory.db` | SQLite database path |
| `embedding_url` | `http://127.0.0.1:1337/v1` | Osaurus embeddings endpoint |
| `embedding_model` | `nomic-embed-text` | Model name for embeddings |

## Full example

See `docs/superpowers/specs/2026-07-18-entheai-hybrid-coding-agent-design.md` §5.22.
