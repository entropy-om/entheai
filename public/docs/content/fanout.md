---
id: fanout
title: "Fan-out & sub-agent roles"
navTitle: "Fan-out & roles"
group: Concepts
order: 3
---

Roles include `explore`, `coder`, `reviewer`, `test`, and `docs`. Each coder runs in its own isolated `git worktree` so parallel work never collides; a run always gets at least one `coder` sub-task, even if the orchestrator's plan comes back explore-only.
