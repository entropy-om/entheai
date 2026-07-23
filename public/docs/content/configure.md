---
id: configure
title: "Configure entheai.toml"
group: "Getting started"
order: 2
---

entheai reads `entheai.toml` from the working directory. Point it at a local Osaurus model or a cloud provider.

```toml
default_model = "osaurus/qwen3-coder"

[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"

[providers.zen]
base_url = "https://opencode.ai/zen/v1"
api_key_env = "OPENCODE_API_KEY"

[router]
orchestrator = "zen/deepseek-v4-pro"
max_turns = 200
max_parallel = 4

[permission]
mode = "ask"   # ask · yolo · plan · auto
```

## Core keys

| Key | Type | Description |
|---|---|---|
| default_model | model id | Model used when `--model` isn't passed. |
| router.orchestrator | model id | Model used for planning / fan-out decomposition. |
| router.max_turns | int | Tool-dispatch turn cap per run (default 200, unlimited under `--yolo`). |
| router.max_parallel | int | Max parallel worktrees during `--fanout` (default 4). |
| permission.mode | enum | ask · yolo · plan · auto |
| providers.\<name\>.base_url | string | OpenAI-compatible endpoint for that provider. |
| providers.\<name\>.api_key_env | string | Env var name to read the API key from (not the key itself). |
