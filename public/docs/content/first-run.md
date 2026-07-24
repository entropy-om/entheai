---
id: first-run
title: "First run"
group: "Getting started"
order: 3
---

Start using `entheai` out-of-the-box with zero API key configuration by connecting to the free community node on `coder.vaked.dev`:

```bash
# Create an initial entheai.toml
cat > entheai.toml <<'TOML'
default_model = "vaked/coder"

[providers.vaked]
base_url = "https://coder.vaked.dev/v1"
TOML

# Run a one-shot query
entheai "summarize this repository"

# Launch the interactive TUI
entheai

# Run parallel fan-out coders in isolated worktrees
entheai --fanout "add a CONTRIBUTING.md and .editorconfig"
```

> [!NOTE]
> On startup, `entheai` indexes codebase symbols into the `codebase` memory namespace. Subsequent runs retrieve relevant architectural spans instantly before model calls.

## Troubleshooting Startup Limits

- **Headless / No-TTY Execution**: If launched without an interactive terminal (e.g. piped stdout), `entheai` names the limit and remedy: run in an interactive terminal, pass `--prompt "<task>"` for one-shot mode, or wrap execution in a pseudo-terminal (`script -q /dev/null entheai`).
