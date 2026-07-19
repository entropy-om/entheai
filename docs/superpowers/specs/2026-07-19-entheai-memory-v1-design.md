# entheai Memory Layer v1 — Design

**Date:** 2026-07-19 · **Status:** approved design, pre-plan
**Scope:** make the **local** memory layer v1-complete. The `codebase` namespace / `codebase-memory-mcp` is its **own** pillar and out of scope here.

## Purpose

Turn `entheai-memory` from an off-by-default, flat-cosine store into a **v1-complete local memory**: an embedded ANN store, **hybrid + scored** recall, an **on-by-default** runtime used across *every* agent path (one-shot, TUI, fan-out), and operator visibility. The agent reliably recalls the *relevant* prior learnings / trajectories / tool-results and improves over time.

## Scope

**In:** the four **local** namespaces (`learnings`, `trajectories`, `tools`, `subagents`) — embedded ANN store (`sqlite-vec`) + keyword (FTS5), hybrid recall (RRF + score + recency-decay), on-by-default runtime wired everywhere, an inspection command.
**Out (explicit):** `codebase` namespace / `codebase-memory-mcp` (own pillar); jcode-style **graph** memory (v1.1); a cross-encoder **reranker** (v1.1); self-learning **routing** (v0.3).

## 1. Store — embedded ANN via `sqlite-vec`

Keep the `Memory` trait + `SqliteStore` (WAL, `spawn_blocking` for all DB I/O). Replace the brute-force flat cosine loop with **`sqlite-vec`** (a loadable SQLite extension providing a `vec0` virtual table with KNN search).

- **Extension load:** register `sqlite-vec` on every connection via the `sqlite-vec` Rust crate's bundled `sqlite3_vec_init` through `rusqlite`'s loadable-extension hook (`unsafe { Connection::load_extension_enable() }` + the crate's registration, or `rusqlite::ffi::sqlite3_auto_extension`). **This wiring is the one real technical risk — the plan pins the exact `rusqlite`/`sqlite-vec` version + registration call and gates it with a round-trip test first.**
- **Schema:** keep `entries(namespace, key, content, metadata, embedding BLOB, created_at, PK(namespace,key))`. Add a partitioned vec table:
  ```sql
  CREATE VIRTUAL TABLE vec_entries USING vec0(
      namespace TEXT PARTITION KEY,
      entry_rowid INTEGER,
      embedding FLOAT[DIM]);
  ```
  `store()` upserts the embedding into `vec_entries` for the entry; `delete()` removes it.
- **Search:** `SELECT entry_rowid, distance FROM vec_entries WHERE namespace = ? AND embedding MATCH ? ORDER BY distance LIMIT ?` — per-namespace ANN — then join `entries` for content/metadata. (No more O(n) scan.)
- **Migration:** on `open()`, create `vec_entries` if absent and **backfill** it from any existing `entries.embedding`.
- `DIM` comes from the embedder (configurable; default 1024 — matches the store constant).

## 2. Recall — hybrid (vector + FTS5) → RRF → score

New `crates/memory/src/recall.rs`. `SqliteStore::search(namespace, query, k)` becomes:

1. **Vector** — embed `query` → `vec0` KNN, over-fetch `3·k`.
2. **Keyword** — SQLite **FTS5** over `content` (`entries_fts` virtual table kept in sync on `store`/`delete`) → BM25 top `3·k`. (`rusqlite` `bundled` ships FTS5.)
3. **Fuse** — **reciprocal-rank fusion**: `rrf(key) = Σ_i 1 / (60 + rank_i)` over the two lists.
4. **Score** — `final = rrf + w_recency · decay(age) + w_conf · confidence`, where `decay(age) = exp(-age_days / half_life)` and `confidence` (0–1) is read from the entry's metadata (default 0.5 when absent). **Decay is a recall-time term — no background job.**
5. Return the top-`k` `ScoredEntry` by `final`.

Graceful degradation: no embedder → **keyword-only**; empty query → empty. Weights/`k`/`half_life` come from config (§4) with defaults `w_recency = 0.3`, `w_conf = 0.2`, `half_life = 14 days`.

## 3. Recording + learning loop

`record_final_answer` (trajectory) and `extract_learnings` stay (with the already-landed char-safe / task-scoped-key / logging / `created_at` fixes). Each learning stores `confidence` + `outcome` (`succeeded`/`failed`/`denied`) in metadata. Contradiction demotion (a `failed` outcome flipping a promoted learning) stays. There is **no background decay job** — staleness is expressed purely through the recall-time recency term, and contradiction through the existing status flip.

## 4. Runtime — on by default, wired everywhere

- `crates/config` `[memory]`: **`enabled = true` by default** (was `false`), plus tuned `retrieve_learnings=6`, `retrieve_trajectories=3`, `tool_spill_chars=8000`, and the recall weights/`half_life`/`k` from §2.
- **Bin wiring:** `bin/entheai` constructs a `SharedMemory` (SQLite at `~/.cache/entheai/memory.db`) + `MemoryRuntime` + a `MemoryScope`, and passes them into the agent runs. Today the memory is off-by-default and may not be constructed in the bin at all — this wires it for the **one-shot** and **TUI** paths (via `run_task_with_memory`).
- **Fan-out sub-agents:** `crates/orchestrator` `run_coder`/`run_subagent` build a `MemoryRuntime` from the same `SharedMemory` (Arc-shared) + a per-sub-agent scope, so sub-agents both **read** collective memory and **write** their trajectories/learnings. Writes stay safe (the store's single `Arc<Mutex<Connection>>` + `spawn_blocking` serializes them).

## 5. Ops — inspection

A `entheai --memory <cmd>` mode (checked before the agent run; runs the query and exits):
- `--memory list <namespace>` — recent entries (key · created_at · content preview).
- `--memory search <namespace> <query>` — the hybrid-scored results.
- `--memory stats` — per-namespace counts + total.

## 6. Architecture / crates

- **`crates/memory`** — `store.rs` (`sqlite-vec` `vec0` + FTS5 + migration/backfill), new `recall.rs` (hybrid + RRF + score), `runtime.rs` (tuned config; logic unchanged); `Cargo.toml` adds `sqlite-vec`.
- **`crates/config`** — `[memory]` defaults (`enabled=true` + weights/`half_life`/`k`).
- **`bin/entheai`** — construct + pass the memory/runtime/scope; the `--memory` inspection mode.
- **`crates/orchestrator`** — sub-agent runs build a `MemoryRuntime` from the shared store.
- `crates/core` — `run_task_with_memory` is unchanged (it already accepts memory).

## 7. Testing

- **store:** `sqlite-vec` ANN round-trip (store N embeddings → KNN returns the nearest, per-namespace) against a real embedded store; FTS5 keyword match; migration backfill (old `entries` → `vec_entries`).
- **recall (pure):** RRF fusion (two ranked lists → correct fused order); the score/recency-decay math (`decay(0)=1`, monotone decreasing); hybrid end-to-end ranking with a mock embedder + FTS5 (a keyword-only hit and a vector-only hit both surface).
- **runtime/config:** `[memory] enabled` defaults `true`; a fan-out sub-agent reads + writes the shared store (integration).
- **ops:** `--memory list/search/stats` against a temp store.

## 8. Success criteria

- The store uses `sqlite-vec` ANN (not an O(n) scan); a ~10k-entry namespace searches in ms.
- `search` returns **hybrid** (vector + keyword) results **fused + scored** by relevance + recency + confidence.
- Memory is **on by default** and used in **one-shot, TUI, and fan-out sub-agents** (they read *and* write it).
- `entheai --memory search learnings "cargo test"` prints relevant stored learnings.
- All previously-landed correctness fixes (char-safe preview, task-scoped keys, embedder timeout, `created_at`, mutex-poison recovery) are preserved.
