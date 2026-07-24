---
id: loop
title: "The agent loop"
group: Concepts
order: 1
badgeText: Concepts
---

The core execution engine (`crates/core`) is powered by `EntheaiAgent`, wrapping [adk-rust](https://github.com/zavora-ai/adk-rust)'s `LlmAgentBuilder`, `Runner`, and `SessionService`.

```text
┌─────────────────────────────────────────────────────────┐
│                    PERCEIVE & RECALL                    │
│  - Pre-task memory retrieval (5-namespace SQLite)        │
│  - Frozen node activation (frozen/*.md trigger match)   │
│  - Raw span mesh re-ranking & Marqant compression       │
└────────────────────────────┬────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────┐
│                    PLAN & GENERATE                      │
│  - Model streaming via adk-rust (OpenAI-compatible)     │
│  - Tool call dispatch generation                        │
└────────────────────────────┬────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────┐
│                    ACT & PERMISSION                     │
│  - Policy check (ask / plan / auto / yolo)              │
│  - Tool execution (read, write, edit, shell, search)    │
│  - Tool-output spillover (large outputs -> memory)      │
└────────────────────────────┬────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────┐
│                    VERIFY & INGEST                      │
│  - Trajectory recording & BrainJudge relevance update    │
│  - Dynamic experience-weighted rank updates overlay     │
└─────────────────────────────────────────────────────────┘
```

## Built-In Tools

- **`read_file`**: Path-sandboxed file content reader.
- **`write_file`**: Creates new files in root-canonicalized paths.
- **`edit_file`**: Precise string replacement in existing files.
- **`run_shell`**: Isolated sub-process execution (`kill_on_drop`, 120s timeout, 100 MB cap).
- **`search`**: Root-scoped pattern search.
