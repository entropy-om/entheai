---
id: configure
title: "Configure entheai.toml"
group: "Getting started"
order: 2
---

`entheai` resolves its config in this order: `./entheai.toml` (or `--config <path>`), then `~/.config/entheai/entheai.toml`, then `~/.config/entheai/config.toml`, then built-in defaults. All keys have sensible defaults; the built-in defaults run on DeepSeek V4 (`DEEPSEEK_API_KEY`). The `deepseek`, `gemini`, `openrouter` and keyless `vaked` providers are always injected, so you only need `[providers.*]` blocks for extra endpoints or overrides. Full key reference: `docs/configuration.md` in the repository.

```toml
default_model = "deepseek/deepseek-v4-flash"

[providers.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key_env = "DEEPSEEK_API_KEY"

[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"

[router]
orchestrator = "deepseek/deepseek-v4-pro"
max_parallel = 4
max_turns = 200

[agents.coder]
model = ["deepseek/deepseek-v4-pro", "gemini/gemini-3.1-pro-preview", "openrouter/deepseek/deepseek-v4-pro"]

[permission]
mode = "ask"   # ask · yolo · plan · auto

[tools]
shell_output_cap = 100000      # 100 KB
search_max_results = 200

[memory]
mode = "prompt-processing"     # "prompt-processing" | "topk"

[current]
enabled = true
refresh_minutes = 120

[viz]
theme = "entheia"   # entheia · ember · verdant · void
brain = true
swarm = true
```

## Complete Configuration Reference

| Key | Type | Default | Description |
|---|---|---|---|
| `default_model` | string | `deepseek/deepseek-v4-flash` | Default model when `--model` is omitted (interactive runs). |
| `providers.<name>.base_url` | string | *required* | OpenAI-compatible endpoint URL. |
| `providers.<name>.api_key_env` | string | `None` | Environment variable name holding the API key (omitted for keyless nodes). |
| `router.orchestrator` | string | `deepseek/deepseek-v4-pro` | Model used by the fan-out orchestrator for task planning and decomposition. |
| `agents.<role>.model` | array | built-in tier | Preference-ordered fallback chain per role (`explore`, `coder`, `reviewer`, `test`, `docs`); the first entry whose provider is available wins, else the orchestrator chain, else the built-in chain (`deepseek-v4-pro` -> Gemini pro -> OpenRouter for coder/reviewer, `deepseek-v4-flash` -> Gemini flash -> OpenRouter for the rest). |
| `router.max_parallel` | int | `8` | Maximum concurrent sub-agents in fan-out mode. |
| `router.max_turns` | int | `200` | Tool-dispatch turn limit per task (`u32::MAX` under `--yolo`). |
| `fanout.verify` | string | `None` | Explicit verification shell command (auto-detects `./scripts/check.sh` if omitted). |
| `fanout.verify_required` | bool | `true` | Requires passing verification before merging coder branches. |
| `fanout.coder_timeout_secs` | int | `600` | Per-coder timeout before aborting a hung sub-agent. |
| `fanout.executor` | string | `"auto"` | Coder backend: `"auto"` (federation when `[federation].enabled` and a worker answers, else local), `"local"` (always in-process on the `[agents.*]` models, never federates), `"agy"` (every coder runs on the Antigravity CLI with `fanout.agy_model`, bypassing `[agents.coder]`), or `"copilot"` (GitHub Copilot CLI). |
| `fanout.agy_model` | string | `"gemini-3.6-flash-high"` | Gemini model the `"agy"` executor runs coders on. |
| `oracle.model` | string | `deepseek/deepseek-v4-pro` | Oracle model (falls through the built-in pro chain, then the free tier, when its provider is unavailable). |
| `permission.mode` | enum | `"ask"` | Permission posture: `"ask"`, `"plan"`, `"auto"`, `"yolo"`. |
| `permission.pins` | table | `{}` | Per-tool pins (e.g. `run_shell = "always_ask"`, `read_file = "always_allow"`). |
| `tools.shell_output_cap` | int | `100000` | Max bytes captured from shell stdout/stderr (100 KB). |
| `tools.search_max_results` | int | `200` | Maximum results returned by file search. |
| `memory.mode` | enum | `"topk"` | Memory pipeline mode: `"prompt-processing"` or `"topk"`. |
| `current.enabled` | bool | `false` | Enable live current-awareness ingestion (Valyu + WorldMonitor + HF dogfood). |
| `current.refresh_minutes` | int | `120` | Cadence for automatic background current-awareness updates in TUI. |
| `chenno.enabled` | bool | `false` | Enable karmapa-chenno context publishing on `/freeze`. |
| `kin.nodes` | array | `[]` | Status URLs for sibling kin nodes rendered in the Zen field. |
| `viz.theme` | enum | `"entheia"` | Zen field ambient theme: `"entheia"`, `"ember"`, `"verdant"`, `"void"`. |
| `viz.brain` | bool | `true` | Always-on braille brain panel in TUI. |
