# Configuration Reference

Configuration lives in `entheai.toml`. Resolution order:

1. `./entheai.toml` in the working directory (or `--config <path>`; an explicit path must exist).
2. `~/.config/entheai/entheai.toml`, then `~/.config/entheai/config.toml` — the per-user global config.
3. Built-in defaults: DeepSeek V4 Flash interactive, V4 Pro orchestrator, gemini + openrouter fallbacks (needs `DEEPSEEK_API_KEY`; fan-out still degrades to the keyless vaked node).

Every key is optional. Unknown keys are silently ignored (there is no `deny_unknown_fields`), so a misspelled key does nothing — this page lists the keys that exist. Provider keys come from the environment; `.env` in the cwd and `~/.config/entheai/entheai.env` are loaded at startup.

The repo's own [`entheai.toml`](../entheai.toml) is a complete, current example.

## `[providers.<name>]`

```toml
[providers.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key_env = "DEEPSEEK_API_KEY"

[providers.gemini]
base_url = "https://generativelanguage.googleapis.com/v1beta/openai"   # unused for kind = "gemini"
api_key_env = "GEMINI_API_KEY"
kind = "gemini"   # native Gemini client — required for Gemini 3.x tool calls

[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"

[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"
# No api_key_env needed for local inference

[providers.quantal]
base_url = ""
kind = "ternary"
model_dir = "/path/to/ternary/model"
```

| Key | Required | Description |
|---|---|---|
| `base_url` | yes | OpenAI-compatible API base (include `/v1`; empty for `kind = "ternary"`) |
| `api_key_env` | no | Environment variable holding the API key (the name, never the key). Unset = keyless |
| `kind` | no | `"openai"` (default), `"gemini"` (adk-rust's native Gemini client; use it for Gemini 3.x — the OpenAI-compatible endpoint cannot complete tool-call turns because the `thought_signature` is not echoed) or `"ternary"` (native 1.58-bit runner over `model_dir`) |
| `model_dir` | no | Directory of the ternary model; only read when `kind = "ternary"` |

**Built-in providers.** entheai-config injects `deepseek`, `gemini` (`kind = "gemini"`), `openrouter` (exactly as above) and the keyless `vaked` free tier (`https://coder.vaked.dev/v1`) into every parsed config, so `deepseek/deepseek-v4-*` resolves with zero `[providers]` blocks as soon as `DEEPSEEK_API_KEY` is set. A user-declared `[providers.<name>]` always wins over the injected one.

## `default_model`

```toml
default_model = "deepseek/deepseek-v4-flash"
```

Format: `<provider>/<model>`, split on the first `/` (so `openrouter/deepseek/deepseek-v4-flash` is provider `openrouter`, model `deepseek/deepseek-v4-flash`). The model for interactive / one-shot runs when `--model` is not passed. Unset: `deepseek/deepseek-v4-flash` (also the built-in config's value). Note it also joins the fan-out chains right after `[router].orchestrator`.

## `[inference]`

```toml
[inference]
temperature = 0.3
max_tokens = 2048
```

| Key | Default | Description |
|---|---|---|
| `temperature` | provider default | Sampling temperature for every call |
| `max_tokens` | provider default | Max output tokens per call |
| `request_timeout_secs` | 120 | Not applied to the HTTP client (adk-rust 1.0.0 gap); used as the 2x per-event idle timeout while consuming a stream |
| `retries` | 2 | Inert with adk-rust 1.0.0 |

## `[router]`

```toml
[router]
orchestrator = "deepseek/deepseek-v4-pro"
max_parallel = 8
max_turns = 200
```

| Key | Default | Description |
|---|---|---|
| `orchestrator` | `default_model`, else `deepseek/deepseek-v4-pro` | Model that plans, decomposes and synthesizes a fan-out |
| `max_parallel` | 8 | Max sub-agents running concurrently |
| `max_turns` | 200 | Tool-dispatch turns per agent before cut-off (`u32::MAX` under `--yolo`) |
| `orchestrator_prompt` | built-in | Replace the orchestrator system prompt |
| `orchestrator_prompt_append` | – | Text appended to the orchestrator system prompt |

The orchestrator chain is walked on availability: `[router].orchestrator` → `default_model` → the built-in pro chain (`deepseek/deepseek-v4-pro`, `gemini/gemini-3.1-pro-preview`, `openrouter/deepseek/deepseek-v4-pro`); the first whose provider is declared **and** whose `api_key_env` is set wins. Note that a configured `default_model` outranks the built-in pro tier — set `[router].orchestrator` explicitly to keep planning on V4 Pro while chatting on Flash.

## `[agents.<role>]`

```toml
[agents.explore]
model = ["deepseek/deepseek-v4-flash", "gemini/gemini-3.6-flash", "openrouter/deepseek/deepseek-v4-flash"]

[agents.coder]
model = ["deepseek/deepseek-v4-pro", "gemini/gemini-3.1-pro-preview", "openrouter/deepseek/deepseek-v4-pro"]
```

Roles the orchestrator emits: `explore`, `coder`, `reviewer`, `test`, `docs`.

| Key | Description |
|---|---|
| `model` | Preference-ordered fallback chain. The first entry whose provider is available (declared + key present) wins. |

When no entry is usable — or the role has no list — the role falls back to the orchestrator chain above (`[router].orchestrator`, `default_model`), then to its built-in chain: the pro chain (`deepseek/deepseek-v4-pro` → `gemini/gemini-3.1-pro-preview` → `openrouter/deepseek/deepseek-v4-pro`) for `coder` / `reviewer`, the flash chain (`deepseek/deepseek-v4-flash` → `gemini/gemini-3.6-flash` → `openrouter/deepseek/deepseek-v4-flash`) for `explore` / `test` / `docs`. On the fan-out path only, a model whose provider is still unavailable degrades to the keyless free tier `vaked/qwen3-coder:30b` (logged as a warning); interactive runs surface the misconfiguration loudly instead.

## `[fanout]`

```toml
[fanout]
executor = "auto"
verify_required = true
coder_timeout_secs = 600
# verify = "cargo test"
```

| Key | Default | Description |
|---|---|---|
| `executor` | `"auto"` | `"auto"`: federation when `[federation].enabled` and a worker answers, else local. `"local"`: always in-process on the `[agents.<role>]` models (never federates). `"agy"`: every coder runs on the Antigravity CLI with `agy_model` (Gemini) — bypasses `[agents.coder]`. `"copilot"`: GitHub Copilot CLI with `copilot_model`. Any other value is a config error. |
| `verify` | auto-detect `./scripts/check.sh` | Shell command run in each coder worktree; a passing run gates integration. Bounded to `coder_timeout_secs`. When auto-detected (not set explicitly), the script is pinned to its content at the run's base commit — not the coder's own worktree copy — so a coder can't fake a pass by editing its own gate; an explicit `verify` command is always trusted as configured. |
| `verify_required` | `true` | When no verify command resolves, leave changed branches unmerged instead of integrating unverified |
| `coder_timeout_secs` | 600 | Per-coder timeout before it is force-aborted |
| `agy_model` | `gemini-3.6-flash-high` | Model passed to `agy --model` (agy's own naming, not `<provider>/<model>`) |
| `copilot_model` | `""` | Model passed to `copilot --model`; empty = the CLI's default |
| `mode` | `""` | Permission-mode override for fan-out sub-agents (`""` = inherit parent ceiling) |

## `[oracle]`

```toml
[oracle]
enabled = false
coders = "local"
gate = "advisory"
block_confidence = 0.8
model = "deepseek/deepseek-v4-pro"
```

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Master switch for the adjudication seam |
| `coders` | `"local"` | Reserved: `"local"` or `"fleet"` (Linux host, eBPF sphere attestation); not read by the runtime yet |
| `gate` | `"advisory"` | `"advisory"` (log only) or `"block"` (block high-confidence Reject/Rework) |
| `block_confidence` | 0.8 | Confidence above which `gate = "block"` blocks |
| `model` | `deepseek/deepseek-v4-pro` | Adjudicator model; when its provider is unavailable falls through the built-in pro chain (Gemini, OpenRouter), then the free tier |

## `[permission]`

| Key | Default | Description |
|---|---|---|
| `yolo` | `false` | Allow every tool call |
| `allowlist` | `[]` | Tool names allowed without asking |
| `fanout_auto_approve` | `true` | Fan-out sub-agents never prompt |
| `mode` | `"ask"` | `ask` / `auto` / `yolo` / `plan` ceiling |
| `pins` | `{}` | Per-tool overrides, e.g. `run_shell = "always_ask"` |

## `[companion]`

```toml
[companion]
enabled = true
always_on_top = true
port = 9876
fps = 24.0
```

Spawns the floating QR-code beacon window (`entheai-companion` binary in PATH). `--no-companion` disables it for a session.

## `[memory]`

```toml
[memory]
enabled = true
path = "~/.cache/entheai/memory.db"
embed_provider = "osaurus"        # a [providers.<name>] key; unset = no embeddings
embed_model = "nomic-embed-text"
```

| Key | Default | Description |
|---|---|---|
| `enabled` | `true` | 5-namespace SQLite memory |
| `strict` | `false` | Memory failures interrupt the task instead of degrading to log diagnostics |
| `path` | `~/.cache/entheai/memory.db` | SQLite database path |
| `embed_provider` | – | Provider whose `base_url` serves `/embeddings` |
| `embed_model` | `nomic-embed-text` | Embedding model name |
| `retrieve_codebase` / `retrieve_learnings` / `retrieve_trajectories` | 4 / 6 / 3 | Recall counts per namespace |
| `max_context_chars` | 12000 | Cap on injected memory context |
| `mode` | `"topk"` | `"topk"` or `"prompt-processing"` (reads `[memory.prompt_processing]`) |

## Other tables

`[mcp.<name>]` (`command`, `args`), `[mcp_defaults]` (`spawn_timeout_secs` = 10: spawn + handshake + tools/list; `call_timeout_secs` = 300: one tools/call), `[skills]` (`dirs`), `[tools]` (`shell_timeout_secs`, `shell_output_cap`, `search_max_results`), `[viz]`, `[telemetry]` (`sentry_dsn`; `""` disables Sentry, an invalid DSN logs a warning and disables it instead of crashing), `[obsidian]`, `[nats]` (`enabled`, `url_env`, `token_env` — the URL/token are read from the environment, never from TOML), `[federation]` (`enabled`, `deadline_secs`, `sandbox`, `max_concurrent_coders`), `[frozen]`, `[current]`, `[chenno]`, `[kin]`. See `crates/config/src/lib.rs` for their fields and defaults.
