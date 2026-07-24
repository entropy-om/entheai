---
id: permissions
title: "Permission gate & YOLO"
group: Concepts
order: 4
---

All side-effecting tool calls pass through `entheai-permission`.

## Permission Postures (`mode`)

Cycle modes interactively with `Shift+Tab` in the TUI or configure `[permission].mode`:

- **`ask`** (default): Prompts for confirmation before executing mutating actions (writing files, running shell commands).
- **`plan`**: Read-only mode. All file/search reads are approved; writes/shell calls are denied.
- **`auto`**: Approves read and write operations; prompts for shell/network calls.
- **`yolo`**: Auto-approves all tool calls and lifts turn caps (`router.max_turns = u32::MAX`).

## Tool Pins (`[permission].pins`)

Pin individual tools to explicit postures:

```toml
[permission]
mode = "ask"

[permission.pins]
run_shell = "always_ask"
read_file = "always_allow"
write_file = "always_ask"
```

> [!WARNING]
> YOLO mode (`--yolo` flag or `mode = "yolo"`) disables all confirmation prompts. Only use YOLO in disposable git worktrees or containerized sandboxes.
