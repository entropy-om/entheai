---
id: configure
title: "Configure entheai.toml"
group: "Getting started"
order: 2
---

`entheai` reads `entheai.toml` from the current working directory. All keys have sensible defaults.

```toml
default_model = "vaked/coder"

[providers.vaked]
base_url = "https://coder.vaked.dev/v1"

[providers.zen]
base_url = "https://opencode.ai/zen/v1"
api_key_env = "OPENCODE_API_KEY"

[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"

[router]
orchestrator = "zen/deepseek-v4-pro"
max_parallel = 4
max_turns = 200

[permission]
mode = "ask"   # ask · yolo · plan · auto

[tools]
shell_output_cap = 100000000   # 100 MB
search_max_results = 10000

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
| `default_model` | string | `deepseek/deepseek-chat` | Default model when `--model` is omitted. |
| `providers.<name>.base_url` | string | *required* | OpenAI-compatible endpoint URL. |
| `providers.<name>.api_key_env` | string | `None` | Environment variable name holding the API key (omitted for keyless nodes). |
| `router.orchestrator` | string | `deepseek/deepseek-chat` | Model used by the orchestrator for task planning and decomposition. |
| `router.max_parallel` | int | `8` | Maximum concurrent sub-agents in fan-out mode. |
| `router.max_turns` | int | `200` | Tool-dispatch turn limit per task (`u32::MAX` under `--yolo`). |
| `fanout.verify` | string | `None` | Explicit verification shell command (auto-detects `./scripts/check.sh` if omitted). |
| `fanout.verify_required` | bool | `true` | Requires passing verification before merging coder branches. |
| `fanout.coder_timeout_secs` | int | `600` | Per-coder timeout before aborting a hung sub-agent. |
| `fanout.executor` | string | `"auto"` | Coder backend: `"auto"`, `"local"`, or `"agy"` (Antigravity CLI recursive dev). |
| `permission.mode` | enum | `"ask"` | Permission posture: `"ask"`, `"plan"`, `"auto"`, `"yolo"`. |
| `permission.pins` | table | `{}` | Per-tool pins (e.g. `run_shell = "always_ask"`, `read_file = "always_allow"`). |
| `tools.shell_output_cap` | int | `100000000` | Max bytes captured from shell stdout/stderr (100 MB). |
| `tools.search_max_results` | int | `10000` | Maximum results returned by file search. |
| `memory.mode` | enum | `"topk"` | Memory pipeline mode: `"prompt-processing"` or `"topk"`. |
| `current.enabled` | bool | `false` | Enable live current-awareness ingestion (Valyu + WorldMonitor + HF dogfood). |
| `current.refresh_minutes` | int | `120` | Cadence for automatic background current-awareness updates in TUI. |
| `chenno.enabled` | bool | `false` | Enable karmapa-chenno context publishing on `/freeze`. |
| `kin.nodes` | array | `[]` | Status URLs for sibling kin nodes rendered in the Zen field. |
| `viz.theme` | enum | `"entheia"` | Zen field ambient theme: `"entheia"`, `"ember"`, `"verdant"`, `"void"`. |
| `viz.brain` | bool | `true` | Always-on braille brain panel in TUI. |
