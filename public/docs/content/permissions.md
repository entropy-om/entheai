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

> [!WARNING]
> YOLO mode (`--yolo`) auto‑approves every tool call. Use it only in a sandbox or a throwaway worktree.
