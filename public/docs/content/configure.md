---
id: configure
title: "Configure entheai.toml"
group: "Getting started"
order: 2
---

entheai reads `entheai.toml` from the working directory. Point it at a local Osaurus model or a cloud provider.

```entheai.toml
[provider.osaurus]
endpoint = "http://127.0.0.1:11434"

[provider.opencode-zen]
api_key = "env:OPENCODE_ZEN_KEY"

[router]
plan = "deepseek/v4-pro"
code = "osaurus/qwen2.5-coder"
```

## Core keys

| Key | Type | Description |
|---|---|---|
| router.plan | model id | Model used for planning / orchestration. |
| router.code | model id | Default model for coder sub-agents. |
| fanout.max | int | Max parallel worktrees (default 4). |
| permissions.mode | enum | ask · yolo |
