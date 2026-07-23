---
id: memory
title: "Memory — five namespaces"
navTitle: "Memory namespaces"
group: Concepts
order: 5
---

A 5-namespace SQLite + vector store (`entheai-memory`), wired into every run: pre-task retrieval before the model call, tool-output spillover during it, and a trajectory + learnings write after.

| Namespace | Description |
|---|---|
| codebase | Repository structure — symbols, call graph, architecture, ADRs. |
| learnings | Durable facts and preferences — "how we solved X". |
| trajectories | Completed-task summaries: model, tool calls, outcome, extracted learnings. |
| tools | Large tool outputs, spilled here and recalled by pointer when they'd bloat context. |
| subagents | Per-sub-agent scratch and outputs during fan-out. |

On top of raw storage, `entheai-memory-pp` adds prompt-processing (recall → mesh rerank → marqant compression) and **frozen nodes** — curated markdown units that stay dormant until a task's deterministic triggers wake them, plus `BrainJudge`, which proactively surfaces a frozen node from ambient tool activity even when nothing in the prompt itself triggered it.

Inspect the store directly: `entheai --memory stats`, `--memory list <namespace>`, `--memory search <namespace> <query...>`.
