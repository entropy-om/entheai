# Prompt-Processing — SLICE 1 Implementation Plan (Phase-1 raw ingest + `RetrievalMode` switch + exact top-K fallback)

> **⚠️ COORDINATE WITH RAHUL FIRST (`rahulmranga`, CODEOWNERS of `crates/memory`).**
> The spec (`docs/superpowers/specs/2026-07-22-prompt-processing-design.md` §29-36) mandates coordination because this feature extends the memory subsystem, whose standing guardrail is *"don't touch `crates/memory`."* This plan is deliberately shaped so that guardrail is honored **literally**: it changes **zero lines in `crates/memory`**. All new machinery lands in a new sibling crate (`crates/memory-pp`) plus `crates/config`, `crates/core`, and `bin/entheai` — none of which Rahul owns. Still: **share this plan + the spec with Rahul, do the work on a branch, and get his sign-off before merge**, because it reads/writes alongside his store and depends on his public types. If Rahul prefers the spec-literal placement (dispatch inside `MemoryRuntime::retrieve_before`), that is a ~30-line change in his crate; the honest ledger of both options is in the "Ownership" section below.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in prompt-processing retrieval mode to entheai that (1) captures the agent's raw experience — full session transcripts + all tool outputs — into a new append-only, content-addressed raw store (Phase-1 ingest), (2) selects between today's top-K retrieval and the new pipeline via a config switch defaulting to `topk`, and (3) guarantees that when the mode is off, unavailable, empty, erroring, or slow, retrieval is **byte-identical to today's top-K** — a fail-safe exercised now, not deferred. The mesh (`ultra-graph`) and compressor (`marqant`) are in-process **stubs behind the real deadline timeout**; the Python sidecar and `mq` subprocess drop into the same trait seams in Slice 2 with zero upstream change.

**Architecture:** A new crate `entheai-memory-pp` owns `RawStore` (SQLite + FTS5, blake3 content-addressed, retention-pruned), the `MeshSearch`/`Marqant` trait seams with Slice-1 stubs, and a `PromptProcessor` that runs `recall → rerank(timeout) → rehydrate raw → compress`, returning `Ok(Some(brief))` only when the full pipeline produced one and `Ok(None)`/`Err(_)` (the fallback signal) on every non-happy branch. The one live caller — `run_task_with_memory` in `crates/core` (single production call site `bin/entheai/src/main.rs:266`) — gains one `pp: Option<&PromptProcessor>` param, a two-branch dispatch at the existing inject block (`crates/core/src/lib.rs:167-181`) whose fallback arm calls the **unchanged** `MemoryRuntime::retrieve_before`, and two Phase-1 ingest hooks (`:214-220` tool-evidence, `:190-194` final answer). `crates/config` gains `[memory] mode` (a `String`, so no new config→memory dependency edge) + a `PromptProcessingConfig` sub-table. `bin/entheai` builds the processor with Slice-1 stubs and prunes on startup.

**Tech Stack:** Rust, tokio (`spawn_blocking` for DB I/O, `time::timeout` for deadlines), rusqlite + FTS5 (mirrors `crates/memory/src/store.rs`), blake3 (content addressing), async-trait, thiserror, serde. New crate: `entheai-memory-pp`. Crates touched: `entheai-memory-pp` (new), `entheai-config`, `entheai-core`, `entheai` (bin). **`entheai-memory` is NOT touched.**

**Scope:** Slice 1 only. **Delivered:** Phase-1 ingest (transcripts + all tool outputs, content-addressed, idempotent, retention-pruned), the `topk`↔`prompt-processing` switch, and the complete fail-safe. **Deferred to Slice 2 (behind the same stubs/traits, no upstream change):** the real `sidecars/ultragraph/serve.py` stdio-JSON-RPC `rerank` sidecar, the real `mq compress --semantic` subprocess, and the vector arm of `RawStore::recall`. Phase-2/3 ingest (codebase snapshots, obsidian, external) are out of scope.

**Refinement over the spec:** The spec sketches dispatch *inside* `MemoryRuntime::retrieve_before`. This plan relocates dispatch to the one core call site so the fallback is *literally* Rahul's unchanged `retrieve_before` (byte-identity by construction, not re-implementation) and `crates/memory` stays untouched. `RetrievalMode` and `PpError` live in the new crate; config carries `mode` as a `String`.

---

## Ownership ledger (honest accounting)

| Crate | Owner | This plan's diff | Why |
|---|---|---|---|
| `crates/memory-pp` (**new**) | ours | all new code | RawStore, traits+stubs, PromptProcessor, PpError, RetrievalMode |
| `crates/config` | ours | `mode: String` + `PromptProcessingConfig` | no `entheai-memory` dep edge introduced |
| `crates/core` | ours | +1 param, dispatch branch, 2 ingest hooks, 1 pure helper | hotspot — run `get_risk` before editing |
| `bin/entheai` | ours | `build_prompt_processor` + wire | mirrors `build_memory` |
| **`crates/memory`** | **Rahul** | **0 lines** | dispatch moved to core; fallback = his unchanged `retrieve_before`; `PpError` isolated |

**Fallback offer if Rahul wants the spec-literal placement:** add `RetrievalMode` (~6 lines) + a `mode` field on `MemoryRuntimeConfig` + a `pp: Option<Box<dyn PromptRetriever>>` trait-object field (trait defined in `crates/memory`, impl in `crates/memory-pp` — breaks the dep cycle) + the dispatch split in `retrieve_before`. That is ~30-40 lines in his crate vs. 0. Recommend 0; confirm the choice with him on the branch.

---

## Adversarial-review corrections — MUST apply (folded in 2026-07-22)

A completeness critic verified this plan against the spec + live tree. It confirmed the core win
(**0 lines changed in `crates/memory`** — verified: dispatch relocated to core, fallback = Rahul's
unchanged `retrieve_before`). Apply these corrections while implementing:

1. **[BLOCKER · Task 6] Update `MemoryConfig`'s manual `Default` impl.** `MemoryConfig` has a
   hand-written `Default` at `crates/config/src/lib.rs:818-840` listing every field. Adding `mode`
   + `prompt_processing` to the struct without adding them there is a hard compile error (the
   task's own `cargo test -p entheai-config` gate would fail). Also add
   `mode: default_memory_mode(), prompt_processing: None,` to the `Default` impl (~:838).

2. **[Fail-safe · Task 5] One overall deadline around the ENTIRE `retrieve()` body.** The plan wraps
   only `mesh.rerank` + `marqant.compress` in `tokio::time::timeout`. `RawStore::recall`, the
   per-span `get()` rehydrate loop, and `Arc<Mutex<Connection>>` contention (ingest hooks fire on
   the hot path) have no timeout — so a slow recall or a large ingest holding the lock HANGS the
   prompt instead of degrading. Wrap the whole `retrieve()` future in a single
   `tokio::time::timeout(self.deadline, …)` → `Ok(None)` on elapse. The spec requires falling back
   when *slow*, not only when erroring.

3. **[Trait symmetry · Task 4] Give `Marqant::compress` a `deadline: Duration`.** It is deadline-less
   while `MeshSearch::rerank` has one. In Slice 2 `mq` is a subprocess; an outer future-cancel won't
   kill the child → orphaned `mq` on every timeout, or a trait change that breaks the "drop in with
   zero upstream change" promise. Add the param now (symmetric with `rerank`); the impl owns
   `kill_on_drop`.

4. **[Hot-path · Task 2/5] Cap per-ingest bytes in Slice 1.** `ingest_tool` writes unbounded tool
   output verbatim into SQLite + FTS under the lock — the exact thing `crates/tools/shell.rs` caps.
   Add a per-ingest byte cap (config knob) now, matching the capped-reader precedent; don't defer to
   Slice 2.

5. **[Spec "loud" half · Task 5/7] Surface fallback to the caller, not just `warn!`.** Spec §Fail-safe
   requires failures to "surface to the caller with a clear reason (which stage, why)." Emit an
   `AgentEvent` (or structured signal) on fallback carrying the failing stage so a TUI/oneshot user
   learns PP fired, fell back, and where. A log line alone does not satisfy it.

6. **[Broken test · Task 8] Fix the smoke test.** `--root` is not a CLI flag (`Cli` at `main.rs:19`
   has `prompt` + `--config`, no `--root`) — the shown invocation never activates PP. Use
   `cd /tmp/pp-smoke && cargo run -p entheai -- --config /tmp/pp-smoke/entheai.toml "…"`.

7. **[Disclose · Scope] Slice-1 ingest fires for ONESHOT runs only.** PP is wired at the single
   `run_task_with_memory` site (`main.rs:266`); the interactive TUI calls `run_task`
   (`crates/tui/src/lib.rs:646`; standing TODO at `:277`), so interactive work is NOT ingested yet.
   State this as a Slice-1 limitation — wiring the TUI seam is required follow-up (same seam Rahul's
   memory-into-TUI TODO needs).

**Lower-severity (note, don't block):** N+1 rehydrate loop per prompt (add `get_many` in Slice 2);
BM25-only recall in Slice 1 (vector arm deferred → recall quality reduced, acceptable boundary);
retention prunes only at startup; the `sidecar_cmd` default-string divergence; the exact-content
transcript filter; verify `injected_ctx` doesn't trip `unused_assignments` under `-D warnings`.

---

### Task 1: Scaffold `entheai-memory-pp` — crate, `PpError`, `RetrievalMode`

**Files:**
- Create: `crates/memory-pp/Cargo.toml`
- Create: `crates/memory-pp/src/lib.rs`
- Create: `crates/memory-pp/src/error.rs`
- Modify: root `Cargo.toml` (add `"crates/memory-pp"` to `[workspace] members`)

- [ ] **Step 1: Add the crate to the workspace + Cargo.toml**

In the root `Cargo.toml`, add `"crates/memory-pp",` to the `members` array (keep it sorted alongside the other `crates/*` entries).

Create `crates/memory-pp/Cargo.toml` (match the workspace's existing dependency style — path deps for in-repo crates, `workspace = true` for shared ones, exactly as `crates/memory/Cargo.toml` does):

```toml
[package]
name = "entheai-memory-pp"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
entheai-memory = { path = "../memory" }
entheai-providers = { path = "../providers" }
tokio = { workspace = true, features = ["rt", "time"] }
serde = { workspace = true }
serde_json = { workspace = true }
async-trait = { workspace = true }
thiserror = { workspace = true }
rusqlite = { workspace = true }
blake3 = "1"
log = "0.4"

[dev-dependencies]
tempfile = "3"
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "time"] }
```

- [ ] **Step 2: Write `error.rs` (the isolated error type — deliberately NOT merged into Rahul's `MemoryError`)**

```rust
//! Prompt-processing errors. Kept in this crate (never added to
//! `entheai_memory::MemoryError`) so `crates/memory` stays untouched. A `PpError`
//! never escapes the retrieval seam: core catches `Ok(None) | Err(_)` and falls
//! back to top-K. Every DB path maps `spawn_blocking` JoinError and a poisoned
//! lock to a recoverable `PpError` (not a panic-unwind) so *every* failure —
//! including a panicked blocking closure — degrades to the fallback.

#[derive(Debug, thiserror::Error)]
pub enum PpError {
    #[error("raw store: {0}")]
    RawStore(#[from] rusqlite::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("blocking task join: {0}")]
    Join(String),
    #[error("raw store lock poisoned")]
    Lock,
    #[error("mesh unavailable")]
    MeshUnavailable,
    #[error("marqant: {0}")]
    Marqant(String),
}
```

- [ ] **Step 3: Write `lib.rs` with `RetrievalMode` + a failing test**

```rust
//! entheai-memory-pp — the opt-in prompt-processing retrieval pipeline.
//!
//! Keep the past RAW; search the raw space; compress LAST. This crate owns the
//! raw experiential store, the mesh/compressor subprocess seams (stubbed in
//! Slice 1), and the orchestrator. It depends on `entheai-memory` for shared
//! types (`MemoryScope`, `ToolEvidence`); `entheai-memory` never depends on it.
//! See docs/superpowers/specs/2026-07-22-prompt-processing-design.md.

mod error;

pub use error::PpError;

/// Which retrieval implementation `run_task_with_memory` dispatches to.
/// `TopK` is today's behaviour; the default guarantees "off unless set".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetrievalMode {
    #[default]
    TopK,
    PromptProcessing,
}

impl RetrievalMode {
    /// Parse the `[memory] mode` config string. Unknown non-"topk" values warn
    /// and default to `TopK` (fail-safe: a typo can never silently disable top-K).
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "prompt-processing" => RetrievalMode::PromptProcessing,
            "topk" | "" => RetrievalMode::TopK,
            other => {
                log::warn!("unknown memory mode {other:?}; defaulting to topk");
                RetrievalMode::TopK
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parse_and_default() {
        assert_eq!(RetrievalMode::default(), RetrievalMode::TopK);
        assert_eq!(RetrievalMode::parse("topk"), RetrievalMode::TopK);
        assert_eq!(RetrievalMode::parse(""), RetrievalMode::TopK);
        assert_eq!(RetrievalMode::parse("prompt-processing"), RetrievalMode::PromptProcessing);
        assert_eq!(RetrievalMode::parse("bogus"), RetrievalMode::TopK);
    }
}
```

- [ ] **Step 4: Build + test**

Run: `cargo test -p entheai-memory-pp`
Expected: 1 test passes, crate compiles clean. (If the workspace uses a `Cargo.lock`, it updates — stage it in the commit.)

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/memory-pp/Cargo.toml crates/memory-pp/src/lib.rs crates/memory-pp/src/error.rs
git commit -m "feat(memory-pp): scaffold crate + PpError + RetrievalMode switch"
```

---

### Task 2: `RawStore` ingest / get / prune (content-addressed, append-only)

**Files:**
- Create: `crates/memory-pp/src/raw_store.rs`
- Modify: `crates/memory-pp/src/lib.rs` (declare `mod raw_store;` + re-export)

- [ ] **Step 1: Write the failing tests** (bottom of `raw_store.rs`; the file won't compile until Step 3 — intended red)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ingest_get_roundtrip_identical_bytes() {
        let s = RawStore::open_memory().unwrap();
        // Punctuation, quotes, newlines — the raw payload must survive verbatim.
        let payload = "raw \"bytes\" (verbatim)!\nline two";
        let id = s.ingest(RawKind::ToolOutput, payload, None).await.unwrap();
        assert!(id.starts_with("blake3:"));
        let got = s.get(&id).await.unwrap().expect("row exists");
        assert_eq!(got.bytes, payload, "raw store never rewrites the payload");
        assert_eq!(got.kind, RawKind::ToolOutput);
    }

    #[tokio::test]
    async fn reingest_same_bytes_is_idempotent() {
        let s = RawStore::open_memory().unwrap();
        let a = s.ingest(RawKind::Transcript, "same content", None).await.unwrap();
        let b = s.ingest(RawKind::Transcript, "same content", None).await.unwrap();
        assert_eq!(a, b, "content-addressed id is stable");
        assert_eq!(s.count().await.unwrap(), 1, "re-ingest is a no-op");
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let s = RawStore::open_memory().unwrap();
        assert!(s.get("blake3:deadbeef").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn prune_respects_retention() {
        let s = RawStore::open_memory().unwrap();
        s.ingest(RawKind::Transcript, "keep-me", None).await.unwrap();
        assert_eq!(s.prune(90).await.unwrap(), 0, "recent row retained");
        assert_eq!(s.count().await.unwrap(), 1);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        assert_eq!(s.prune(0).await.unwrap(), 1, "cutoff=now drops older-than-now");
        assert_eq!(s.count().await.unwrap(), 0);
    }
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-memory-pp raw_store` → compile error (`RawStore` not found).

- [ ] **Step 3: Implement `raw_store.rs`** (above the test module). The `Arc<Mutex<Connection>>` + `spawn_blocking` discipline mirrors `crates/memory/src/store.rs:58-124`; JoinError and lock poisoning map to `PpError` so a panicked closure degrades to fallback rather than unwinding.

```rust
//! The raw experiential tier (Stage 1). A separate SQLite DB (never one of
//! Rahul's five `Namespace`s) so `mode="topk"` is byte-identical: this surface
//! is wholly disjoint. Append-only, content-addressed (blake3), retention-scoped.
//! The stored bytes are NEVER lossily rewritten — only the FTS index is derived.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::error::PpError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawKind {
    Transcript,
    ToolOutput,
    // Slice 2/3: CodebaseSnapshot, ObsidianNote, External
}

impl RawKind {
    fn as_str(self) -> &'static str {
        match self {
            RawKind::Transcript => "transcript",
            RawKind::ToolOutput => "tool_output",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "transcript" => Some(RawKind::Transcript),
            "tool_output" => Some(RawKind::ToolOutput),
            _ => None,
        }
    }
}

/// A recall hit: locates a raw passage, never carries the full payload.
#[derive(Debug, Clone)]
pub struct RawSpan {
    pub id: String,
    pub kind: RawKind,
    pub score: f32,
    pub created_at: i64,
}

/// The full, never-rewritten raw payload, retrieved by content id.
#[derive(Debug, Clone)]
pub struct RawContent {
    pub id: String,
    pub kind: RawKind,
    pub bytes: String,
    pub meta: Option<serde_json::Value>,
    pub created_at: i64,
}

pub struct RawStore {
    db: Arc<Mutex<Connection>>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS raw (
             id         TEXT PRIMARY KEY,
             kind       TEXT NOT NULL,
             bytes      TEXT NOT NULL,
             meta       TEXT,
             created_at INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS idx_raw_created ON raw(created_at);
         CREATE VIRTUAL TABLE IF NOT EXISTS raw_fts USING fts5(id UNINDEXED, bytes);",
    )
}

impl RawStore {
    pub fn open(path: &Path) -> Result<Self, PpError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        ensure_schema(&conn)?;
        Ok(Self { db: Arc::new(Mutex::new(conn)) })
    }

    pub fn open_memory() -> Result<Self, PpError> {
        let conn = Connection::open_in_memory()?;
        ensure_schema(&conn)?;
        Ok(Self { db: Arc::new(Mutex::new(conn)) })
    }

    /// Append-only, content-addressed, idempotent. Re-ingesting identical
    /// (kind, bytes) is a no-op that returns the existing id (spec §151).
    pub async fn ingest(
        &self,
        kind: RawKind,
        bytes: &str,
        meta: Option<serde_json::Value>,
    ) -> Result<String, PpError> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(kind.as_str().as_bytes());
        hasher.update(&[0u8]);
        hasher.update(bytes.as_bytes());
        let id = format!("blake3:{}", hasher.finalize().to_hex());

        let db = self.db.clone();
        let id_ret = id.clone();
        let bytes = bytes.to_string();
        let meta_txt = match meta {
            Some(v) => Some(serde_json::to_string(&v)?),
            None => None,
        };
        let created_at = now_ms();

        tokio::task::spawn_blocking(move || -> Result<(), PpError> {
            let conn = db.lock().map_err(|_| PpError::Lock)?;
            let changed = conn.execute(
                "INSERT OR IGNORE INTO raw (id, kind, bytes, meta, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![id, kind.as_str(), bytes, meta_txt, created_at],
            )?;
            // Only mirror into FTS when the row was actually inserted, so the
            // standalone FTS table can't accumulate duplicates on re-ingest.
            if changed > 0 {
                conn.execute(
                    "INSERT INTO raw_fts (id, bytes) VALUES (?1, ?2)",
                    rusqlite::params![id, bytes],
                )?;
            }
            Ok(())
        })
        .await
        .map_err(|e| PpError::Join(e.to_string()))??;

        Ok(id_ret)
    }

    /// The full raw payload by content id — byte-identical to what was ingested.
    pub async fn get(&self, span_id: &str) -> Result<Option<RawContent>, PpError> {
        let db = self.db.clone();
        let span_id = span_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<RawContent>, PpError> {
            let conn = db.lock().map_err(|_| PpError::Lock)?;
            let mut stmt = conn
                .prepare("SELECT id, kind, bytes, meta, created_at FROM raw WHERE id = ?1")?;
            let mut rows = stmt.query(rusqlite::params![span_id])?;
            match rows.next()? {
                Some(row) => {
                    let kind_s: String = row.get(1)?;
                    let meta_txt: Option<String> = row.get(3)?;
                    Ok(Some(RawContent {
                        id: row.get(0)?,
                        kind: RawKind::parse(&kind_s).unwrap_or(RawKind::ToolOutput),
                        bytes: row.get(2)?,
                        meta: meta_txt.and_then(|t| serde_json::from_str(&t).ok()),
                        created_at: row.get(4)?,
                    }))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| PpError::Join(e.to_string()))?
    }

    /// Delete rows older than `retention_days`; returns the count removed.
    pub async fn prune(&self, retention_days: u64) -> Result<usize, PpError> {
        let cutoff = now_ms() - (retention_days as i64) * 86_400_000;
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<usize, PpError> {
            let mut conn = db.lock().map_err(|_| PpError::Lock)?;
            let tx = conn.transaction()?;
            tx.execute(
                "DELETE FROM raw_fts WHERE id IN (SELECT id FROM raw WHERE created_at < ?1)",
                rusqlite::params![cutoff],
            )?;
            let n = tx.execute("DELETE FROM raw WHERE created_at < ?1", rusqlite::params![cutoff])?;
            tx.commit()?;
            Ok(n)
        })
        .await
        .map_err(|e| PpError::Join(e.to_string()))?
    }

    pub async fn count(&self) -> Result<usize, PpError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<usize, PpError> {
            let conn = db.lock().map_err(|_| PpError::Lock)?;
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM raw", [], |r| r.get(0))?;
            Ok(n as usize)
        })
        .await
        .map_err(|e| PpError::Join(e.to_string()))?
    }
}
```

- [ ] **Step 4: Declare + re-export** in `crates/memory-pp/src/lib.rs` (after `mod error;`):

```rust
mod raw_store;

pub use raw_store::{RawContent, RawKind, RawSpan, RawStore};
```

- [ ] **Step 5: Run, verify pass** — `cargo test -p entheai-memory-pp raw_store` → 4 tests pass.

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy -p entheai-memory-pp -- -D warnings`

```bash
git add crates/memory-pp/src/raw_store.rs crates/memory-pp/src/lib.rs
git commit -m "feat(memory-pp): RawStore — content-addressed ingest/get/prune (append-only, idempotent)"
```

---

### Task 3: `RawStore::recall` — FTS5 lexical recall + query sanitizer

**Files:**
- Modify: `crates/memory-pp/src/raw_store.rs` (add `sanitize_fts5_query` + `recall`; extend tests)

- [ ] **Step 1: Write the failing tests** (add to the `tests` module in `raw_store.rs`)

```rust
    #[tokio::test]
    async fn recall_finds_by_keyword_and_rehydrates() {
        let s = RawStore::open_memory().unwrap();
        s.ingest(RawKind::Transcript, "the auth login flow", None).await.unwrap();
        s.ingest(RawKind::ToolOutput, "unrelated disk usage report", None).await.unwrap();
        let spans = s.recall("auth", 10).await.unwrap();
        assert_eq!(spans.len(), 1, "only the matching span");
        assert_eq!(spans[0].kind, RawKind::Transcript);
        let rc = s.get(&spans[0].id).await.unwrap().unwrap();
        assert_eq!(rc.bytes, "the auth login flow", "span id rehydrates raw payload");
    }

    #[tokio::test]
    async fn recall_respects_k() {
        let s = RawStore::open_memory().unwrap();
        for i in 0..5 {
            s.ingest(RawKind::Transcript, &format!("auth event number {i}"), None).await.unwrap();
        }
        assert_eq!(s.recall("auth", 3).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn recall_punctuated_query_does_not_error() {
        let s = RawStore::open_memory().unwrap();
        s.ingest(RawKind::Transcript, "fix the auth bug in v2", None).await.unwrap();
        // A raw FTS5 MATCH of this string is a syntax error (quotes, parens, `?`);
        // the sanitizer must turn it into a valid, recall-preserving query. Without
        // this, PP would silently never fire for punctuated prompts.
        let spans = s.recall("fix the \"auth\" bug (v2)?", 10).await.unwrap();
        assert!(!spans.is_empty(), "punctuated prompt still recalls");
    }

    #[test]
    fn sanitize_rejects_empty_and_quotes_tokens() {
        assert_eq!(sanitize_fts5_query("   "), None);
        assert_eq!(sanitize_fts5_query("a-b c"), Some("\"a\" OR \"b\" OR \"c\"".to_string()));
    }
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-memory-pp raw_store` → `sanitize_fts5_query`/`recall` not found.

- [ ] **Step 3: Implement** (add to `raw_store.rs`, above the `impl RawStore` block for the free fn, and a `recall` method inside `impl RawStore`)

```rust
/// Turn arbitrary user prompt text into a syntactically valid FTS5 MATCH
/// expression. Split on non-alphanumerics, quote each token (so FTS operators
/// `"`, `*`, `-`, `:`, `(`, `AND`/`OR`/`NEAR` can't be misparsed), join with OR
/// to maximise recall. Returns None when there is nothing to match on.
pub(crate) fn sanitize_fts5_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}
```

Add inside `impl RawStore`:

```rust
    /// Stage-1 lexical recall (FTS5/BM25). Returns candidate spans best-first,
    /// ≤ `k`. The vector arm is Slice 2 (same signature). An unmatchable query
    /// yields an empty Vec — which the processor treats as "fall back to top-K".
    pub async fn recall(&self, query: &str, k: usize) -> Result<Vec<RawSpan>, PpError> {
        let Some(match_expr) = sanitize_fts5_query(query) else {
            return Ok(Vec::new());
        };
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<RawSpan>, PpError> {
            let conn = db.lock().map_err(|_| PpError::Lock)?;
            let mut stmt = conn.prepare(
                "SELECT r.id, r.kind, r.created_at, bm25(raw_fts) AS bm
                 FROM raw_fts JOIN raw r ON r.id = raw_fts.id
                 WHERE raw_fts MATCH ?1
                 ORDER BY bm ASC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![match_expr, k as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            })?;
            let mut out = Vec::new();
            for r in rows {
                let (id, kind_s, created_at, bm) = r?;
                if let Some(kind) = RawKind::parse(&kind_s) {
                    // bm25 is lower-is-better (negative); flip so higher = better.
                    out.push(RawSpan { id, kind, score: (-bm) as f32, created_at });
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| PpError::Join(e.to_string()))?
    }
```

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-memory-pp raw_store` → 8 tests pass.

- [ ] **Step 5: Clippy + commit**

Run: `cargo clippy -p entheai-memory-pp -- -D warnings`

```bash
git add crates/memory-pp/src/raw_store.rs
git commit -m "feat(memory-pp): RawStore::recall (FTS5/BM25) + FTS5 query sanitizer"
```

---

### Task 4: `MeshSearch` / `Marqant` seams + Slice-1 stubs

**Files:**
- Create: `crates/memory-pp/src/mesh.rs`
- Create: `crates/memory-pp/src/marqant.rs`
- Modify: `crates/memory-pp/src/lib.rs` (declare + re-export)

- [ ] **Step 1: Write the failing tests** (put a `tests` module at the bottom of each new file)

`mesh.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn stub_mesh_is_unavailable() {
        let r = StubMesh.rerank("q", &[], Duration::from_millis(10)).await;
        assert!(matches!(r, Err(PpError::MeshUnavailable)));
    }

    #[tokio::test]
    async fn slow_mesh_exceeds_deadline() {
        let mesh = SlowStubMesh { sleep: Duration::from_millis(200) };
        let r = tokio::time::timeout(
            Duration::from_millis(20),
            mesh.rerank("q", &[], Duration::from_millis(20)),
        )
        .await;
        assert!(r.is_err(), "the timeout arm fires (elapsed)");
    }

    #[tokio::test]
    async fn identity_mesh_returns_candidates_unchanged() {
        use crate::raw_store::{RawKind, RawSpan};
        let spans = vec![RawSpan { id: "a".into(), kind: RawKind::Transcript, score: 1.0, created_at: 0 }];
        let out = IdentityMesh.rerank("q", &spans, Duration::from_millis(10)).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "a");
    }
}
```

`marqant.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_marqant_is_identity() {
        assert_eq!(StubMarqant.compress("brief body").await.unwrap(), "brief body");
    }
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-memory-pp mesh` → compile error (types not found).

- [ ] **Step 3: Implement `mesh.rs`** (the trait is the Slice-2 subprocess contract; stubs stand behind it now)

```rust
//! Stage 2 — the 1-bit LLM mesh re-rank seam. In Slice 1 this is stubbed
//! in-process; Slice 2 drops in a stdio-JSON-RPC client over the existing
//! `crates/mcp` plumbing (method `rerank`, params {query, spans:[{id,text}],
//! deadline_ms, top_k?}, result {ranked_span_ids: <subset of input ids>, ...}).
//! The sidecar returns IDS ONLY — the Rust side rehydrates raw via RawStore::get,
//! preserving "never returns a rewritten payload".

use std::time::Duration;

use async_trait::async_trait;

use crate::error::PpError;
use crate::raw_store::RawSpan;

#[async_trait]
pub trait MeshSearch: Send + Sync {
    /// Return a subset/reordering of `spans` (never new ids). Errors or a
    /// deadline overrun signal the caller to fall back to top-K.
    async fn rerank(
        &self,
        query: &str,
        spans: &[RawSpan],
        deadline: Duration,
    ) -> Result<Vec<RawSpan>, PpError>;
}

/// Slice-1 production default: always unavailable → deterministic, fast fallback
/// to today's top-K. (No sidecar ships in Slice 1.)
pub struct StubMesh;

#[async_trait]
impl MeshSearch for StubMesh {
    async fn rerank(&self, _q: &str, _spans: &[RawSpan], _d: Duration) -> Result<Vec<RawSpan>, PpError> {
        Err(PpError::MeshUnavailable)
    }
}

/// Test double: sleeps past the deadline to exercise the timeout→fallback arm.
pub struct SlowStubMesh {
    pub sleep: Duration,
}

#[async_trait]
impl MeshSearch for SlowStubMesh {
    async fn rerank(&self, _q: &str, spans: &[RawSpan], _d: Duration) -> Result<Vec<RawSpan>, PpError> {
        tokio::time::sleep(self.sleep).await;
        Ok(spans.to_vec())
    }
}

/// Test double: identity re-rank (candidates unchanged) for happy-path wiring.
pub struct IdentityMesh;

#[async_trait]
impl MeshSearch for IdentityMesh {
    async fn rerank(&self, _q: &str, spans: &[RawSpan], _d: Duration) -> Result<Vec<RawSpan>, PpError> {
        Ok(spans.to_vec())
    }
}
```

- [ ] **Step 4: Implement `marqant.rs`**

```rust
//! Stage 3 — deterministic compression seam (marqant `mq`). Slice-1 stub is an
//! identity passthrough (never reached on the live path — StubMesh short-circuits
//! before it). Slice 2 swaps in the `mq compress <in.md> -o <out.mq> --semantic`
//! subprocess (file-arg I/O, `--semantic` yes / `--binary` no, capped reader +
//! timeout mirroring crates/tools/src/shell.rs; deterministic, golden-testable).

use async_trait::async_trait;

use crate::error::PpError;

#[async_trait]
pub trait Marqant: Send + Sync {
    /// Deterministically distil raw findings into the injectable brief.
    async fn compress(&self, findings: &str) -> Result<String, PpError>;
}

pub struct StubMarqant;

#[async_trait]
impl Marqant for StubMarqant {
    async fn compress(&self, findings: &str) -> Result<String, PpError> {
        Ok(findings.to_string())
    }
}
```

- [ ] **Step 5: Declare + re-export** in `crates/memory-pp/src/lib.rs` (after the `raw_store` re-export):

```rust
mod marqant;
mod mesh;

pub use marqant::{Marqant, StubMarqant};
pub use mesh::{IdentityMesh, MeshSearch, SlowStubMesh, StubMesh};
```

Add `use crate::error::PpError;` at the top of `mesh.rs`/`marqant.rs` test modules only if needed (the tests reference `PpError` via `super::*`, which re-exports it through the `use crate::error::PpError;` already in each file).

- [ ] **Step 6: Run, verify pass** — `cargo test -p entheai-memory-pp` → all tests (mode + raw_store + mesh + marqant) pass.

- [ ] **Step 7: Clippy + commit**

Run: `cargo clippy -p entheai-memory-pp -- -D warnings`

```bash
git add crates/memory-pp/src/mesh.rs crates/memory-pp/src/marqant.rs crates/memory-pp/src/lib.rs
git commit -m "feat(memory-pp): MeshSearch/Marqant seams + Slice-1 stubs (contracts fixed for Slice 2)"
```

---

### Task 5: `PromptProcessor` — pipeline (fallback-first) + Phase-1 ingest

**Files:**
- Create: `crates/memory-pp/src/processor.rs`
- Modify: `crates/memory-pp/src/lib.rs` (declare + re-export)

- [ ] **Step 1: Write the failing tests** (bottom of `processor.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::marqant::StubMarqant;
    use crate::mesh::{IdentityMesh, MeshSearch, SlowStubMesh, StubMesh};
    use crate::raw_store::{RawKind, RawStore};
    use std::time::Duration;

    fn scope() -> entheai_memory::MemoryScope {
        entheai_memory::MemoryScope {
            session_id: "sess".into(),
            task_id: "task".into(),
            cwd: std::path::PathBuf::from("/tmp"),
            role: None,
        }
    }

    fn pp_with(mesh: Box<dyn MeshSearch>) -> PromptProcessor {
        let raw = RawStore::open_memory().unwrap();
        PromptProcessor::new(raw, mesh, Box::new(StubMarqant), Duration::from_millis(50), 16)
    }

    #[tokio::test]
    async fn empty_query_falls_back() {
        let pp = pp_with(Box::new(IdentityMesh));
        assert_eq!(pp.retrieve("   ").await.unwrap(), None);
    }

    #[tokio::test]
    async fn empty_store_falls_back() {
        let pp = pp_with(Box::new(IdentityMesh));
        assert_eq!(pp.retrieve("anything").await.unwrap(), None);
    }

    #[tokio::test]
    async fn stub_mesh_unavailable_falls_back() {
        let pp = pp_with(Box::new(StubMesh));
        pp.raw().ingest(RawKind::Transcript, "the auth thing", None).await.unwrap();
        assert_eq!(pp.retrieve("auth").await.unwrap(), None, "mesh err → fallback signal");
    }

    #[tokio::test]
    async fn slow_mesh_times_out_to_fallback() {
        let pp = pp_with(Box::new(SlowStubMesh { sleep: Duration::from_millis(300) }));
        pp.raw().ingest(RawKind::Transcript, "auth login flow", None).await.unwrap();
        assert_eq!(pp.retrieve("auth").await.unwrap(), None, "deadline → fallback signal");
    }

    #[tokio::test]
    async fn happy_path_produces_brief_from_raw() {
        let pp = pp_with(Box::new(IdentityMesh));
        pp.raw().ingest(RawKind::Transcript, "auth login flow details", None).await.unwrap();
        let brief = pp.retrieve("auth").await.unwrap().expect("brief");
        assert!(brief.contains("auth login flow details"), "brief carries the raw finding");
    }

    #[tokio::test]
    async fn ingest_tool_and_transcript_land_rows() {
        let pp = pp_with(Box::new(StubMesh));
        let ev = entheai_memory::ToolEvidence {
            call_id: "c1".into(),
            name: "run_shell".into(),
            args: "ls".into(),
            result: "file-a\nfile-b".into(),
            allowed: true,
        };
        pp.ingest_tool(&scope(), &ev).await;
        let msgs = vec![entheai_providers::ChatMessage::user("hi")];
        pp.ingest_transcript(&scope(), &msgs, "done").await;
        assert_eq!(pp.raw().count().await.unwrap(), 2, "one tool row + one transcript row");
    }
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-memory-pp processor` → `PromptProcessor` not found.

- [ ] **Step 3: Implement `processor.rs`**

```rust
//! The prompt-processing orchestrator: recall → rerank(timeout) → rehydrate raw
//! → compress. Every non-happy branch returns `Ok(None)` (or propagates `Err`),
//! which the core call site treats as "fall back to today's top-K". Ingest is
//! best-effort (log + swallow) so it can never fail a task.
//!
//! Slice-1 success-path contract (documented now so Slice 2 isn't a silent
//! divergence): the brief is the compressor's output injected VERBATIM as the
//! system-message content. It does NOT reuse top-K's `[label score= key=]`
//! block format — the marqant `.mq` brief is itself the injectable body. In
//! Slice 1 `StubMesh` short-circuits, so the success path never fires in
//! production; the ingest side is what is live and testable.

use std::time::Duration;

use log::warn;
use serde_json::json;

use entheai_memory::{MemoryScope, ToolEvidence};
use entheai_providers::ChatMessage;

use crate::error::PpError;
use crate::marqant::Marqant;
use crate::mesh::MeshSearch;
use crate::raw_store::{RawKind, RawStore};

pub struct PromptProcessor {
    raw: RawStore,
    mesh: Box<dyn MeshSearch>,
    marqant: Box<dyn Marqant>,
    deadline: Duration,
    recall_k: usize,
}

impl PromptProcessor {
    pub fn new(
        raw: RawStore,
        mesh: Box<dyn MeshSearch>,
        marqant: Box<dyn Marqant>,
        deadline: Duration,
        recall_k: usize,
    ) -> Self {
        Self { raw, mesh, marqant, deadline, recall_k }
    }

    /// The ingest hooks reach the raw store through this.
    pub fn raw(&self) -> &RawStore {
        &self.raw
    }

    /// Best-effort retention prune (called once at startup). Never fails a run.
    pub async fn prune(&self, retention_days: u64) {
        if let Err(e) = self.raw.prune(retention_days).await {
            warn!("pp raw prune failed (continuing): {e}");
        }
    }

    /// Produce a brief, or signal "fall back to top-K".
    /// `Ok(Some(brief))` = success; `Ok(None)` / `Err(_)` = fall back.
    pub async fn retrieve(&self, msg: &str) -> Result<Option<String>, PpError> {
        if msg.trim().is_empty() {
            return Ok(None);
        }
        // Stage 1 — cheap, wide lexical recall.
        let candidates = self.raw.recall(msg, self.recall_k).await?;
        if candidates.is_empty() {
            return Ok(None); // empty raw store / no lexical hit → fallback
        }
        // Stage 2 — mesh re-rank, bounded by the deadline. Mesh error OR timeout
        // → Ok(None) (fallback), never Err: an experimental-path failure must not
        // become fatal even under strict mode.
        let ranked = match tokio::time::timeout(
            self.deadline,
            self.mesh.rerank(msg, &candidates, self.deadline),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                warn!("pp mesh error → falling back to top-K: {e}");
                return Ok(None);
            }
            Err(_) => {
                warn!("pp mesh deadline exceeded → falling back to top-K");
                return Ok(None);
            }
        };
        if ranked.is_empty() {
            return Ok(None);
        }
        // Rehydrate RAW payloads by id (never rewritten).
        let mut findings = String::new();
        for s in &ranked {
            if let Some(rc) = self.raw.get(&s.id).await? {
                findings.push_str(&rc.bytes);
                findings.push('\n');
            }
        }
        if findings.is_empty() {
            return Ok(None);
        }
        // Stage 3 — deterministic compression, bounded. Error/timeout/empty brief
        // → fallback (an empty brief must never be injected as "success").
        match tokio::time::timeout(self.deadline, self.marqant.compress(&findings)).await {
            Ok(Ok(brief)) if !brief.trim().is_empty() => Ok(Some(brief)),
            Ok(Ok(_)) => Ok(None),
            Ok(Err(e)) => {
                warn!("pp marqant error → falling back to top-K: {e}");
                Ok(None)
            }
            Err(_) => {
                warn!("pp marqant deadline exceeded → falling back to top-K");
                Ok(None)
            }
        }
    }

    // ---- Phase-1 ingest (unconditional raw capture, best-effort) ----

    /// Tool outputs/diffs — captured RAW and UNCONDITIONALLY (ahead of, and
    /// independent of, Rahul's `should_spill` gate), content-addressed.
    pub async fn ingest_tool(&self, scope: &MemoryScope, ev: &ToolEvidence) {
        let meta = json!({
            "tool": ev.name,
            "call_id": ev.call_id,
            "session": scope.session_id,
            "task": scope.task_id,
            "allowed": ev.allowed,
        });
        if let Err(e) = self.raw.ingest(RawKind::ToolOutput, &ev.result, Some(meta)).await {
            warn!("pp ingest_tool failed (continuing): {e}");
        }
    }

    /// Full session transcript (every turn + the final answer), captured RAW —
    /// the counterpart to `record_final_answer`, which stores previews only.
    /// The caller passes a transcript already cleaned of the injected memory
    /// context (see `transcript_for_ingest` in crates/core) so the raw store is
    /// never contaminated by memory's own injected brief.
    pub async fn ingest_transcript(
        &self,
        scope: &MemoryScope,
        messages: &[ChatMessage],
        final_answer: &str,
    ) {
        let mut buf = String::new();
        for m in messages {
            buf.push_str(&m.role);
            buf.push_str(": ");
            buf.push_str(&m.content);
            buf.push('\n');
        }
        buf.push_str("assistant: ");
        buf.push_str(final_answer);
        buf.push('\n');
        let meta = json!({ "session": scope.session_id, "task": scope.task_id });
        if let Err(e) = self.raw.ingest(RawKind::Transcript, &buf, Some(meta)).await {
            warn!("pp ingest_transcript failed (continuing): {e}");
        }
    }
}
```

- [ ] **Step 4: Declare + re-export** in `crates/memory-pp/src/lib.rs`:

```rust
mod processor;

pub use processor::PromptProcessor;
```

- [ ] **Step 5: Run, verify pass** — `cargo test -p entheai-memory-pp` → all pass (mode + raw_store + mesh + marqant + processor).

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy -p entheai-memory-pp -- -D warnings`

```bash
git add crates/memory-pp/src/processor.rs crates/memory-pp/src/lib.rs
git commit -m "feat(memory-pp): PromptProcessor pipeline (fallback-first) + Phase-1 ingest hooks"
```

---

### Task 6: Config — `[memory] mode` + `[memory.prompt_processing]`

**Files:**
- Modify: `crates/config/src/lib.rs` (`MemoryConfig` struct ~737-773; default fns ~775+; add `PromptProcessingConfig` + defaults; add tests near the existing config tests)

- [ ] **Step 1: Write the failing tests** (add to the config test module — mirror the existing `[memory]`/`[federation]` default tests in this file)

```rust
    #[test]
    fn memory_mode_defaults_to_topk() {
        let cfg = Config::from_toml_str("[memory]\nenabled = true\n").unwrap();
        assert_eq!(cfg.memory.mode, "topk");
        assert!(cfg.memory.prompt_processing.is_none());
    }

    #[test]
    fn memory_prompt_processing_parses() {
        let cfg = Config::from_toml_str(
            "[memory]\nmode = \"prompt-processing\"\n\
             [memory.prompt_processing]\nsearch_deadline_ms = 800\nrecall_k = 32\n",
        )
        .unwrap();
        assert_eq!(cfg.memory.mode, "prompt-processing");
        let pp = cfg.memory.prompt_processing.unwrap();
        assert_eq!(pp.search_deadline_ms, 800);
        assert_eq!(pp.recall_k, 32);
        assert_eq!(pp.marqant_cmd, "mq", "absent sub-fields take their defaults");
        assert_eq!(pp.raw_retention_days, 90);
    }
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-config memory_mode` → no field `mode`.

- [ ] **Step 3: Add the fields + struct + defaults**

In `struct MemoryConfig` (after the `embed_timeout_secs` field at ~772, before the closing `}` at 773):

```rust
    /// Retrieval mode: "topk" (today's behaviour, default) | "prompt-processing".
    #[serde(default = "default_memory_mode")]
    pub mode: String,
    /// Prompt-processing sub-table; only read when `mode = "prompt-processing"`.
    #[serde(default)]
    pub prompt_processing: Option<PromptProcessingConfig>,
```

Add the default fn next to the other memory defaults (~775+):

```rust
fn default_memory_mode() -> String {
    "topk".into()
}
```

Add the new struct + its defaults (after the `MemoryConfig` default fns block):

```rust
/// Prompt-processing configuration (spec §Configuration). All fields default,
/// so `[memory.prompt_processing]` can be omitted entirely.
#[derive(Debug, Clone, Deserialize)]
pub struct PromptProcessingConfig {
    /// The mesh sidecar command (Slice 2; unused by the Slice-1 stub).
    #[serde(default = "default_pp_sidecar_cmd")]
    pub sidecar_cmd: String,
    /// Ternary models in the mesh (Slice 2).
    #[serde(default = "default_pp_mesh_size")]
    pub mesh_size: usize,
    /// Fail fast to fallback past this per-stage deadline.
    #[serde(default = "default_pp_search_deadline_ms")]
    pub search_deadline_ms: u64,
    /// The compression subprocess (Slice 2; unused by the Slice-1 stub).
    #[serde(default = "default_pp_marqant_cmd")]
    pub marqant_cmd: String,
    /// Raw-store retention window; pruned on startup.
    #[serde(default = "default_pp_raw_retention_days")]
    pub raw_retention_days: u64,
    /// Stage-1 lexical recall breadth.
    #[serde(default = "default_pp_recall_k")]
    pub recall_k: usize,
    /// Raw-store DB path (separate file from memory.db).
    #[serde(default = "default_pp_raw_path")]
    pub raw_path: String,
}

impl Default for PromptProcessingConfig {
    fn default() -> Self {
        Self {
            sidecar_cmd: default_pp_sidecar_cmd(),
            mesh_size: default_pp_mesh_size(),
            search_deadline_ms: default_pp_search_deadline_ms(),
            marqant_cmd: default_pp_marqant_cmd(),
            raw_retention_days: default_pp_raw_retention_days(),
            recall_k: default_pp_recall_k(),
            raw_path: default_pp_raw_path(),
        }
    }
}

fn default_pp_sidecar_cmd() -> String {
    // NOTE: the published `ultragraph-1bit` ships no stdio `rerank` module; the
    // Slice-2 sidecar is a new in-repo script. Default points at it.
    "python sidecars/ultragraph/serve.py".into()
}
fn default_pp_mesh_size() -> usize {
    8
}
fn default_pp_search_deadline_ms() -> u64 {
    1500
}
fn default_pp_marqant_cmd() -> String {
    "mq".into()
}
fn default_pp_raw_retention_days() -> u64 {
    90
}
fn default_pp_recall_k() -> usize {
    64
}
fn default_pp_raw_path() -> String {
    "~/.cache/entheai/raw.db".into()
}
```

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-config memory` → both new tests pass, existing `[memory]` default tests still green.

- [ ] **Step 5: Clippy + commit**

Run: `cargo clippy -p entheai-config -- -D warnings`

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): [memory] mode + [memory.prompt_processing] sub-table (defaults to topk)"
```

---

### Task 7: Core — dispatch + Phase-1 ingest hooks in `run_task_with_memory`

> **Hotspot:** `crates/core/src/lib.rs` is the repo's #1 churn hotspot (97.6th %ile). Run `get_risk(targets=["crates/core/src/lib.rs"])` before editing. The edits here are surgical: one new param, one dispatch block replacing `:169`, two ingest calls, one pure helper.

**Files:**
- Modify: `crates/core/Cargo.toml` (add `entheai-memory-pp` path dep)
- Modify: `crates/core/src/lib.rs` (add `pp` param at `:150-159`; a pure `transcript_for_ingest` helper + its test; dispatch at `:167-181`; ingest hooks at `:190-194` and `:214-220`)
- Modify: `bin/entheai/src/main.rs` (update the one caller at `:266` — signature only; full wiring is Task 8)

- [ ] **Step 1: Add the dep** to `crates/core/Cargo.toml` (match the crate's existing path-dep style, alongside `entheai-memory`):

```toml
entheai-memory-pp = { path = "../memory-pp" }
```

- [ ] **Step 2: Write the failing test** for the pure helper (add to the `#[cfg(test)] mod tests` in `crates/core/src/lib.rs`; if none exists, create one at the file bottom)

```rust
    #[test]
    fn transcript_for_ingest_drops_only_injected_ctx() {
        use entheai_providers::ChatMessage;
        let injected = "Memory context:\n\n[codebase score=0.90 key=k]\nbody\n";
        let messages = vec![
            ChatMessage::system("you are helpful"),         // real system prompt — kept
            ChatMessage::system(injected.to_string()),      // memory's injected brief — dropped
            ChatMessage::user("do the thing"),
        ];
        let clean = transcript_for_ingest(&messages, Some(injected));
        assert_eq!(clean.len(), 2);
        assert!(clean.iter().all(|m| m.content != injected), "injected ctx filtered out");
        assert!(clean.iter().any(|m| m.content == "you are helpful"), "real system prompt kept");

        // No injection this turn → nothing dropped.
        let clean2 = transcript_for_ingest(&messages, None);
        assert_eq!(clean2.len(), 3);
    }
```

- [ ] **Step 3: Run, verify fail** — `cargo test -p entheai-core transcript_for_ingest` → `transcript_for_ingest` not found.

- [ ] **Step 4: Add the pure helper** (near the other free fns in `crates/core/src/lib.rs`, e.g. beside `truncate_preview`)

```rust
/// Build the transcript to raw-ingest, excluding the memory-context system
/// message we injected before the user turn (identified by exact content match).
/// Without this, `ingest_transcript` would re-ingest memory's own injected brief,
/// which would then be recalled and compressed into future briefs — a
/// self-reinforcing loop that degrades retrieval quality across sessions.
fn transcript_for_ingest(
    messages: &[entheai_providers::ChatMessage],
    injected_ctx: Option<&str>,
) -> Vec<entheai_providers::ChatMessage> {
    messages
        .iter()
        .filter(|m| !(m.role == "system" && injected_ctx == Some(m.content.as_str())))
        .cloned()
        .collect()
}
```

- [ ] **Step 5: Add the `pp` param** to `run_task_with_memory` (`:150-159`). Insert after the `memory` param (the fn already has `#[allow(clippy::too_many_arguments)]`):

```rust
        memory: Option<&entheai_memory::MemoryRuntime>,
        pp: Option<&entheai_memory_pp::PromptProcessor>,
        scope: entheai_memory::MemoryScope,
```

- [ ] **Step 6: Replace the retrieval dispatch** (`:167-181`). Track the injected ctx so Step 8 can clean the transcript. Replace the block from `if let Some(user_idx) = ...` through its closing `}` with:

```rust
            if let Some(user_idx) = messages.iter().rposition(|m| m.role == "user") {
                let user_msg = messages[user_idx].content.clone();
                // Dispatch: prompt-processing when configured+present; else today's
                // top-K. The fallback arm calls the UNCHANGED retrieve_before with
                // the SAME query, so a fallback is byte-identical to today.
                let retrieved: Result<Option<String>, entheai_memory::MemoryError> = match pp {
                    Some(p) => match p.retrieve(&user_msg).await {
                        Ok(Some(brief)) => Ok(Some(brief)),
                        Ok(None) | Err(_) => mem.retrieve_before(&user_msg).await,
                    },
                    None => mem.retrieve_before(&user_msg).await,
                };
                match retrieved {
                    Ok(Some(ctx)) => {
                        injected_ctx = Some(ctx.clone());
                        messages.insert(user_idx, entheai_providers::ChatMessage::system(ctx));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        if mem.config().strict {
                            return Err(CoreError::Memory(e.to_string()));
                        }
                    }
                }
            }
```

Declare `injected_ctx` just before the `if let Some(mem) = memory {` guard at `:161` (so it is in scope at the final-answer hook regardless of the guard):

```rust
        let mut injected_ctx: Option<String> = None;
```

- [ ] **Step 7: Tool-output ingest hook** — inside the tool loop where `ev` is built (`:214-220`), immediately after the `let ev = ... ;` literal and **before** `mem.record_tool_result` at `:221`, add:

```rust
                    // Phase-1 raw ingest: unconditional, ahead of the spill gate.
                    if let Some(p) = pp {
                        p.ingest_tool(&scope, &ev).await;
                    }
```

(`ev` is not moved until the later `tool_evidence.push(ev)`, so borrowing it here is fine.)

- [ ] **Step 8: Transcript ingest hook** — in the final-answer branch (`:190-194`), after the existing `record_final_answer` call and before `return Ok(resp.content);` at `:201`, add:

```rust
                if let Some(p) = pp {
                    let clean = transcript_for_ingest(&messages, injected_ctx.as_deref());
                    p.ingest_transcript(&scope, &clean, &resp.content).await;
                }
```

- [ ] **Step 9: Update the one caller** at `bin/entheai/src/main.rs:266-274` — add the new arg (Task 8 replaces `None` with the real processor):

```rust
                    .run_task_with_memory(
                        messages,
                        &registry,
                        &policy,
                        &mut prompter,
                        None,
                        runtime.as_ref(),
                        None, // pp — wired in Task 8
                        scope,
                    )
```

- [ ] **Step 10: Run, verify pass**

Run: `cargo test -p entheai-core transcript_for_ingest` → passes.
Run: `cargo build -p entheai-core && cargo build -p entheai` → clean (the caller compiles with `None`).

- [ ] **Step 11: Clippy + commit**

Run: `cargo clippy -p entheai-core -- -D warnings`

```bash
git add crates/core/Cargo.toml crates/core/src/lib.rs bin/entheai/src/main.rs
git commit -m "feat(core): PP dispatch (fallback→unchanged top-K) + Phase-1 ingest hooks + clean-transcript guard"
```

---

### Task 8: Binary wiring — build + attach the processor, prune on startup

**Files:**
- Modify: `bin/entheai/Cargo.toml` (add `entheai-memory-pp` path dep)
- Modify: `bin/entheai/src/main.rs` (`build_prompt_processor` mirroring `build_memory` at `:467-493`; build + prune + pass `pp.as_ref()` at the call site `:256-274`)

- [ ] **Step 1: Add the dep** to `bin/entheai/Cargo.toml` (match the existing path-dep style):

```toml
entheai-memory-pp = { path = "../../crates/memory-pp" }
```

- [ ] **Step 2: Add `build_prompt_processor`** near `build_memory` (~`:463`):

```rust
/// Build the prompt-processing pipeline when `[memory] mode = "prompt-processing"`
/// and memory is enabled. Slice 1 uses the in-process stubs (StubMesh /
/// StubMarqant) — retrieval always falls back to top-K, but the raw tier ingests
/// live. Returns `None` for the default `topk` mode. Mirrors `build_memory`.
fn build_prompt_processor(
    cfg: &Config,
) -> anyhow::Result<Option<entheai_memory_pp::PromptProcessor>> {
    use entheai_memory_pp::{PromptProcessor, RetrievalMode, RawStore, StubMarqant, StubMesh};

    if !cfg.memory.enabled || RetrievalMode::parse(&cfg.memory.mode) != RetrievalMode::PromptProcessing {
        return Ok(None);
    }
    let pc = cfg.memory.prompt_processing.clone().unwrap_or_default();
    let path = expand_home(&pc.raw_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let raw = RawStore::open(&path)?;
    let pp = PromptProcessor::new(
        raw,
        Box::new(StubMesh),      // Slice 2: stdio-JSON-RPC sidecar client
        Box::new(StubMarqant),   // Slice 2: `mq compress --semantic` subprocess
        std::time::Duration::from_millis(pc.search_deadline_ms),
        pc.recall_k,
    );
    Ok(Some(pp))
}
```

- [ ] **Step 3: Build + prune at startup, then attach at the call site.** In the oneshot arm (~`:256`), after `let runtime = ...;` and before the `run_task_with_memory` call, add:

```rust
                let pp = build_prompt_processor(&cfg)?;
                if let Some(p) = &pp {
                    let retention = cfg
                        .memory
                        .prompt_processing
                        .as_ref()
                        .map(|c| c.raw_retention_days)
                        .unwrap_or(90);
                    p.prune(retention).await;
                }
```

Then change the `None, // pp` arg in the `run_task_with_memory` call (from Task 7 Step 9) to:

```rust
                        pp.as_ref(),
```

- [ ] **Step 4: Build + clippy**

Run: `cargo build -p entheai && cargo clippy -p entheai -- -D warnings` → clean.

- [ ] **Step 5: Manual smoke — topk unchanged, prompt-processing populates raw.db**

Run (default mode is byte-identical to today):

```bash
cargo run -p entheai -- --root /tmp/pp-smoke "say hi"
```

Then enable PP via a temp config and confirm the raw store fills:

```bash
mkdir -p /tmp/pp-smoke
cat > /tmp/pp-smoke/entheai.toml <<'EOF'
[memory]
enabled = true
mode = "prompt-processing"
[memory.prompt_processing]
raw_path = "/tmp/pp-smoke/raw.db"
EOF
cargo run -p entheai -- --root /tmp/pp-smoke "run: echo hello, then tell me what you did"
# Expect: normal answer (retrieval fell back to top-K via StubMesh), AND:
sqlite3 /tmp/pp-smoke/raw.db "SELECT kind, count(*) FROM raw GROUP BY kind;"
# Expect at least: transcript|1  and (if a tool ran) tool_output|N
```

- [ ] **Step 6: Commit**

```bash
git add bin/entheai/Cargo.toml bin/entheai/src/main.rs
git commit -m "feat(bin): build+attach PromptProcessor (Slice-1 stubs) + startup prune; default topk unchanged"
```

---

### Task 9: Integration guard + docs + workspace gate

**Files:**
- Modify: `crates/memory-pp/src/processor.rs` (add one `#[ignore]` end-to-end fallback-identity test)
- Modify: `entheai.toml` (document the knobs)
- Modify: `CHANGELOG.md` (Unreleased → Added)

- [ ] **Step 1: Add the end-to-end fallback-identity guard** (in the `processor.rs` tests module). Proves the Slice-1 invariant: with the production stub, PP over a populated store still yields the fallback signal (so core injects the unchanged top-K result), and ingest is idempotent across a re-run.

```rust
    #[tokio::test]
    #[ignore = "integration: exercised in the full suite / CI gate"]
    async fn slice1_end_to_end_falls_back_and_ingest_is_idempotent() {
        use crate::mesh::StubMesh;
        let pp = pp_with(Box::new(StubMesh)); // production stub

        // Simulate a run's ingest.
        let sc = scope();
        let msgs = vec![
            entheai_providers::ChatMessage::user("fix the auth bug"),
        ];
        pp.ingest_transcript(&sc, &msgs, "fixed it").await;
        let ev = entheai_memory::ToolEvidence {
            call_id: "c".into(), name: "run_shell".into(), args: "grep auth".into(),
            result: "auth.rs:42".into(), allowed: true,
        };
        pp.ingest_tool(&sc, &ev).await;
        assert_eq!(pp.raw().count().await.unwrap(), 2);

        // Retrieval with the production stub → fallback signal (core uses top-K).
        assert_eq!(pp.retrieve("auth").await.unwrap(), None);

        // Re-running the same session ingests nothing new (content-addressed).
        pp.ingest_transcript(&sc, &msgs, "fixed it").await;
        pp.ingest_tool(&sc, &ev).await;
        assert_eq!(pp.raw().count().await.unwrap(), 2, "idempotent across re-runs");
    }
```

Run: `cargo test -p entheai-memory-pp -- --ignored slice1_end_to_end` → passes.

- [ ] **Step 2: Document the knobs in `entheai.toml`** (add a commented block; PP is off by default):

```toml
# ── Prompt-processing retrieval (opt-in) ──────────────────────────────────────
# [memory]
# mode = "prompt-processing"        # default: "topk" (today's behaviour)
# [memory.prompt_processing]
#   search_deadline_ms = 1500       # fail fast to top-K past this per-stage deadline
#   raw_retention_days = 90         # raw experiential store retention (pruned at startup)
#   recall_k           = 64         # Stage-1 lexical recall breadth
#   raw_path           = "~/.cache/entheai/raw.db"
#   # Slice 2 (not yet active): sidecar_cmd, mesh_size, marqant_cmd
```

- [ ] **Step 3: `CHANGELOG.md`** — add under `## [Unreleased]` → `### Added`:

```markdown
- **Prompt-processing retrieval — Slice 1 (opt-in, `[memory] mode = "prompt-processing"`).** A new raw experiential tier (`crates/memory-pp`): full session transcripts and all tool outputs are captured RAW, content-addressed (idempotent), and retention-pruned. Retrieval runs recall → mesh re-rank → deterministic compression, but the mesh (`ultra-graph`) and compressor (`marqant`) are in-process stubs behind a strict per-stage deadline in Slice 1 — so retrieval always falls back cleanly to today's top-K, byte-identical, whenever PP is off, empty, erroring, or slow. Default `topk` behaviour is unchanged. (Slice 2 drops the real Python sidecar + `mq` subprocess into the same trait seams.) Zero changes to `crates/memory`.
```

- [ ] **Step 4: Commit**

```bash
git add crates/memory-pp/src/processor.rs entheai.toml CHANGELOG.md
git commit -m "test(memory-pp): Slice-1 end-to-end fallback+idempotency guard; docs + CHANGELOG"
```

---

## Final verification (after all tasks)

- [ ] `cargo test -p entheai-memory-pp -p entheai-config -p entheai-core` — all green.
- [ ] `cargo test -p entheai-memory-pp -- --ignored` — the end-to-end guard passes.
- [ ] `cargo test -p entheai-memory` — **Rahul's crate is untouched; his existing tests still pass unchanged** (the byte-identity guarantee: the fallback arm calls his unmodified `retrieve_before`).
- [ ] `cargo clippy --workspace -- -D warnings` — clean.
- [ ] `cargo build --no-default-features -p entheai` — headless build still compiles (PP pulls in no GUI deps; the sidecar/`mq` are subprocesses, not crate deps).
- [ ] `git diff main --stat -- crates/memory/` — **empty** (confirm zero lines changed in Rahul's crate).
- [ ] Manual: `mode = "topk"` run is byte-identical to today; `mode = "prompt-processing"` run answers identically (fallback) **and** populates `raw.db` with transcript + tool_output rows; a second run of the same session adds no rows.
- [ ] Share the branch + this plan with `rahulmranga` for sign-off before merge (spec §29-36), noting the zero-line `crates/memory` diff and the Slice-2 seam markers (`Box::new(StubMesh)` / `Box::new(StubMarqant)` in `build_prompt_processor`, and the unused `sidecar_cmd`/`mesh_size`/`marqant_cmd` config fields).
- [ ] Dispatch a holistic code-reviewer subagent over the whole Slice-1 diff.

## Deferred to Slice 2 (same seams, no upstream change)
- Real `sidecars/ultragraph/serve.py` stdio-JSON-RPC `rerank` sidecar → swap `Box::new(StubMesh)` for the `crates/mcp`-hosted client (newline-delimited JSON-RPC 2.0, `u64` ids, 8 MiB line cap, handshake+per-request timeout, `ChildGuard` kill-on-drop; ids-only result rehydrated via `RawStore::get`).
- Real `mq compress <in.md> -o <out.mq> --semantic` subprocess → swap `Box::new(StubMarqant)` (file-arg I/O, capped reader + timeout per `crates/tools/src/shell.rs:61-99`; pin the binary, disable DNS dictionary resolution; golden-testable).
- Vector arm of `RawStore::recall` (reuse `entheai_memory::Embedder` + a vec table alongside `raw_fts`), and a batched `RawStore::get_many` to replace the per-span `get` loop in `PromptProcessor::retrieve`.
