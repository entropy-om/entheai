---
id: permissions
title: "Permission gate & YOLO"
group: Concepts
order: 4
---

By default every side‑effecting tool call asks for approval.

```text
allow run_shell("cargo test")? [y/N]
```

Four modes (`[permission].mode` in `entheai.toml`, or Shift+Tab to cycle in the TUI): `ask` (default — prompt every time), `plan` (allow reads, deny writes), `auto` (allow up to the exec tier, deny network), `yolo` (allow everything).

> [!WARNING]
> YOLO mode (`--yolo` or `mode = "yolo"`) auto‑approves every tool call and lifts the turn cap entirely. Use it only in a sandbox or a throwaway worktree.
