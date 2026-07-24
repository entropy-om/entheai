---
id: memory
title: "Memory — five namespaces"
navTitle: "Memory namespaces"
group: Concepts
order: 5
---

`entheai` manages memory across five dedicated SQLite + vector namespaces (`entheai-memory`):

| Namespace | Description |
|---|---|
| `codebase` | Codebase structure — symbol indexing, call graph, and architecture. |
| `learnings` | Durable facts and user preferences — "how we solved X". |
| `trajectories` | Task transcripts, tool call tracebacks, and verification outcomes. |
| `tools` | Large tool output spillover recalled via pointer. |
| `subagents` | Per-sub-agent scratchpad and intermediate outputs during fan-out. |

## Prompt-Processing Pipeline (`entheai-memory-pp`)

When `[memory] mode = "prompt-processing"` is enabled:

1. **Stage 1 (Raw Experiential Store)**: Append-only raw storage of transcripts and tool outputs.
2. **Stage 2 (Native 1-Bit LLM Mesh Search)**: In-process ternary BitNet re-ranking (`entheai-ultragraph`) running a trained 768-d linear reranker (`reranker.ugm` — 94.7% accuracy).
3. **Stage 3 (Marqant Structure-Preserving Compression)**: Deterministic context pruning (`kompress-core`) enforcing must-keep overrides for CamelCase, ALLCAPS, hex, CLI flags, and paths.

## Frozen Nodes & Dynamic Re-Ranking

- **Curated Doctrine**: Markdown units in `frozen/*.md` (NixOS, Rust, GitHub, Valyu, etc.) sit dormant until deterministic triggers match.
- **`BrainJudge`**: Background task judging recent tool activity to surface relevant frozen nodes proactively even without exact prompt keywords.
- **Experience Overlay (`frozen-ranks.json`)**: Outcomes dynamically re-weight priors: verified success `+0.02`, verify failure `-0.05` (clamped `[0, 2.0]`). Original `.md` files are never rewritten.
- **`/freeze` & `/thaw`**: Snapshot live entropy state to `.entheai/checkpoints/<id>.json` and rehydrate surviving spans into injected context briefs.
- **`[current]` Awareness Ingestion**: Live feeds from Valyu, WorldMonitor (clamped $\le 50$/day), and HF `ultrawhale-dogfood` Q&A batches flow into the raw memory soil.

Inspect memory via CLI: `entheai --memory stats`, `entheai --memory list <namespace>`, `entheai --memory search <namespace> <query>`.
