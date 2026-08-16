---
id: first-run
title: "First run"
group: "Getting started"
order: 3
---

Out of the box `entheai` runs on DeepSeek V4: `deepseek/deepseek-v4-flash` interactively and `deepseek/deepseek-v4-pro` as the fan-out orchestrator. No `entheai.toml` is needed; export `DEEPSEEK_API_KEY` and go:

```bash
export DEEPSEEK_API_KEY=sk-...

# Run a one-shot query
entheai "summarize this repository"

# Launch the interactive TUI
entheai

# Run parallel fan-out coders in isolated worktrees
entheai --fanout "add a CONTRIBUTING.md and .editorconfig"
```

You can also start with zero API key configuration by connecting to the free community node on `coder.vaked.dev` (`vaked` is built in, no `[providers.vaked]` block needed):

```bash
# Interactive keyless run on the free tier
entheai --model vaked/qwen3-coder:30b "summarize this repository"

# Fan-out degrades to vaked/qwen3-coder:30b automatically when DEEPSEEK_API_KEY is unset
entheai --fanout "add a CONTRIBUTING.md and .editorconfig"
```

> [!NOTE]
> On startup, `entheai` indexes codebase symbols into the `codebase` memory namespace. Subsequent runs retrieve relevant architectural spans instantly before model calls.

## Troubleshooting Startup Limits

- **Headless / No-TTY Execution**: If launched without an interactive terminal (e.g. piped stdout), `entheai` names the limit and remedy: run in an interactive terminal, pass `--prompt "<task>"` for one-shot mode, or wrap execution in a pseudo-terminal (`script -q /dev/null entheai`).
