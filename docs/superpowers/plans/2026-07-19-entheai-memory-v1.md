# entheai Memory Layer v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `entheai-memory` into a v1-complete local memory — embedded ANN via `sqlite-vec`, hybrid (vector + FTS5) recall fused by RRF and scored by recency/confidence, on-by-default and wired into one-shot / TUI / fan-out sub-agents, plus a `--memory` inspection command.

**Architecture:** Keep the `Memory` trait + `SqliteStore` (WAL, `spawn_blocking`, `Arc<Mutex<Connection>>`, mutex-poison recovery). Replace the O(n) cosine scan with three synced SQLite structures — a rowid `entries` table (join key), an `sqlite-vec` `vec0` KNN table, and an FTS5 keyword table. `SqliteStore::search` runs both arms, fuses ids with reciprocal-rank fusion, then scores by `rrf + w_recency·decay(age) + w_conf·confidence`. The store is constructed in `bin/entheai` and threaded into every agent path via the existing `run_task_with_memory`.

**Tech Stack:** Rust, `rusqlite` (bundled SQLite, ships FTS5), `sqlite-vec` (loadable extension registered process-wide via `sqlite3_auto_extension`), `tokio` `spawn_blocking`.

---

> ## Execution status (2026-07-19) — engine + inspection CLI DONE, wiring (Tasks 9–10) handed to @rahulmranga
>
> **Tasks 0–7 complete and reviewed** (two-stage: spec + code-quality), each committed to `main`:
> - Task 0 `12588b7`/`97838e5` · Task 1 `9f9600f`/`527dc7f`/`fb02bc4` · Task 2 `93dc3ae`/`cb31d9b` · Task 3 `b087b1c`/`aa445ba` · Task 4 `855b899` · Task 5 `05059ad`/`392501a` · Task 6 `0d1e2fe` · Task 7 `ad543e4`.
> - The store (sqlite-vec ANN + FTS5 + migration + transactional tri-store sync), `recall.rs` (RRF **normalized to [0,1]** + recency/confidence), config (on-by-default + weights), and the **one-shot** bin path are done and green in isolation.
>
> **Task 8 DONE** (`af4461a`, 2026-07-19) — the `--memory list/search/stats` inspection CLI landed on green `main`.
>
> **Tasks 9–10 REASSIGNED to Rahul (@rahulmranga) (2026-07-19).** Per the maintainer, the remaining memory "brain" wiring is Rahul's to complete. Ownership is tagged in `.github/CODEOWNERS` (`/crates/memory/` → @rahulmranga) and via inline `TODO(@rahulmranga)` markers at the exact seams — `crates/tui/src/lib.rs` (`run`) for Task 9 and `crates/orchestrator/src/lib.rs` (`run_fanout`) for Task 10. The verbatim recipes remain in Task 9 / Task 10 below.
>
> **Task 10 signature drift — MUST adapt on resume:** the fan-out session changed `run_fanout` to take a `WorkerPool`: it is now `run_fanout(config, root, task, events, pool)` (pool = `entheai_orchestrator::WorkerPool::new(cfg.router.max_parallel.max(1))`). Task 10's plan text assumes `run_fanout(config, root, task, events, memory)`. On resume, Task 10 becomes **`run_fanout(config, root, task, events, pool, memory: Option<SharedMemory>)`** — add `memory` as an *additional* trailing arg, do not remove `pool`; update the call sites in `bin/entheai/src/main.rs` (fanout branch) and `crates/tui/src/lib.rs` accordingly, and thread `memory` down through `run_fanout_readonly`/`run_coder`/`run_subagent` per the plan. Coordinate with whoever owns the orchestrator so the arg order is agreed.
>
> **Also on resume:** the TUI call site `crates/tui/src/lib.rs` and `bin/main.rs`'s fanout branch may already be fixed by the fan-out session — re-read both before editing. Task 9 (TUI memory) additionally threads `Option<Arc<MemoryRuntime>>` + `MemoryScope` into `tui::run`.

---

## Scope

**In:** `crates/memory` (store: `vec0` + FTS5 + migration/backfill; new `recall.rs`; `store_inner`/`search_hybrid` seams), `crates/config` (`[memory]` on-by-default + recall weights + path), `bin/entheai` (construct + wire the store, one-shot path, `--memory` mode), `crates/tui` (thread memory into the run loop), `crates/orchestrator` (fan-out sub-agents build a `MemoryRuntime` from the shared store).

**Out (unchanged):** `codebase` namespace / `codebase-memory-mcp`, jcode graph memory, cross-encoder reranker, self-learning routing. `crates/core::run_task_with_memory` is already correct and is **not** modified.

## Repo hazard — read before every commit

This is the shared multi-session `main` checkout. Every task ends with a commit that uses **scoped `git add <exact paths>`** (never `git add -A`/`.`) and **pushes immediately**:

```bash
git add <exact paths for this task>
git commit -m "<message>"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

Never touch files outside a task's listed paths. Do not run `git reset --hard` (it destroys other sessions' un-pushed commits). Follow strict SemVer — this is additive feature work on the `0.x` line; do not bump `[workspace.package].version`.

## Design decisions locked here (reconciling the spec)

1. **`entries` becomes a rowid table** (`id INTEGER PRIMARY KEY`, `UNIQUE(namespace,key)`), not `WITHOUT ROWID`. `vec0` and FTS5 both join on that integer `id`. Task 1 migrates any pre-v1 `WITHOUT ROWID` DB in place.
2. **DIM is derived from the embedder at runtime** (spec §1: "DIM comes from the embedder"), not a fixed `1024` constant. The `vec0` table is created lazily at the first embedding's length and remembered in a `meta` table. A later embedding of a different length is skipped (logged) so a model change degrades to keyword-only instead of crashing. This removes a whole class of DIM-misconfiguration bugs and lets tests use tiny vectors.
3. **The embedder stays optional.** On-by-default is safe offline: with no `embed_provider` configured the store runs keyword-only (FTS5 needs no network). Vector search + embedding writes only happen when the user configures an embed provider.
4. **Two internal seams** make the DB path fully testable without network: `store_inner(…, embedding: Option<Vec<f32>>)` (all writes) and `search_hybrid(…, query_embedding: Option<&[f32]>, query_text, k)` (both recall arms). The public `store`/`search` just embed, then call these.

## File structure

| File | Responsibility |
|------|----------------|
| `crates/memory/Cargo.toml` | add `sqlite-vec` dependency |
| `crates/memory/src/store.rs` | extension registration, rowid schema + migration, `entries_fts`, lazy `vec_entries`, `store_inner`, `search_hybrid`, `RecallParams` |
| `crates/memory/src/recall.rs` | **new** — pure RRF fusion, recency decay, final-score math |
| `crates/memory/src/lib.rs` | `mod recall;` + re-exports (`RecallParams`); `ScoredEntry` doc |
| `crates/memory/src/runtime.rs` | unchanged logic; verify preserved fixes |
| `crates/config/src/lib.rs` | `[memory]` `enabled=true` default, recall weights, global path |
| `bin/entheai/src/main.rs` | build the store/embedder/runtime/scope; one-shot via `run_task_with_memory`; `--memory` mode |
| `crates/tui/src/lib.rs` + `Cargo.toml` | thread `Option<Arc<MemoryRuntime>>` + `MemoryScope` into the run loop |
| `crates/orchestrator/src/lib.rs` + `Cargo.toml` | fan-out leaves build a `MemoryRuntime` from the shared store |

---

## Task 0: sqlite-vec extension wiring + KNN gate test (de-risk first)

This is the one real technical risk (spec §1). Pin the dependency and prove a `vec0` KNN round-trip works against rusqlite's bundled SQLite **before** building anything on it.

**Files:**
- Modify: `crates/memory/Cargo.toml`
- Modify: `crates/memory/src/store.rs`
- Modify: `Cargo.lock` (regenerated)

- [ ] **Step 1: Add the dependency**

Run: `cargo add sqlite-vec -p entheai-memory` (resolves to a `0.1.x`). Confirm `crates/memory/Cargo.toml` `[dependencies]` now contains a line like:

```toml
sqlite-vec = "0.1"
```

Record the exact resolved version from `Cargo.lock` in the commit message. `rusqlite` stays as the workspace `bundled` build — `sqlite-vec` links against that same bundled SQLite. No extra `rusqlite` feature is needed: we call the FFI `sqlite3_auto_extension`, not `Connection::load_extension`.

- [ ] **Step 2: Write the failing gate test**

Add to the `#[cfg(test)] mod tests` block in `crates/memory/src/store.rs`:

```rust
#[test]
fn vec0_knn_roundtrip_gate() {
    // Registers sqlite-vec, then proves a vec0 KNN query returns the nearest
    // neighbour — using the little-endian f32 BLOB representation production uses.
    ensure_vec_extension();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE v USING vec0(
             namespace text partition key,
             embedding float[4] distance_metric=cosine);",
    )
    .unwrap();

    let rows: [(i64, &str, [f32; 4]); 3] = [
        (1, "learnings", [1.0, 0.0, 0.0, 0.0]),
        (2, "learnings", [0.0, 1.0, 0.0, 0.0]),
        (3, "learnings", [0.0, 0.0, 1.0, 0.0]),
    ];
    for (id, ns, vec) in rows {
        conn.execute(
            "INSERT INTO v(rowid, namespace, embedding) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, ns, f32_slice_to_blob(&vec)],
        )
        .unwrap();
    }

    let query = f32_slice_to_blob(&[0.9, 0.1, 0.0, 0.0]);
    let nearest: i64 = conn
        .query_row(
            "SELECT rowid FROM v
             WHERE namespace = ?1 AND embedding MATCH ?2 AND k = ?3
             ORDER BY distance",
            rusqlite::params!["learnings", query, 1_i64],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(nearest, 1, "row 1 is closest to the query vector");
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p entheai-memory vec0_knn_roundtrip_gate`
Expected: FAIL to compile — `cannot find function ensure_vec_extension in this scope`.

- [ ] **Step 4: Implement the registration**

Add near the top of `crates/memory/src/store.rs` (below the `use` lines):

```rust
use std::sync::Once;

/// Register the `sqlite-vec` loadable extension for every SQLite connection
/// opened in this process. Idempotent (guarded by `Once`) — safe to call from
/// each `SqliteStore` constructor. Uses the FFI `sqlite3_auto_extension`
/// (canonical sqlite-vec/rusqlite wiring) so no per-connection load is needed.
fn ensure_vec_extension() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: `sqlite3_vec_init` has the sqlite3 extension-entry ABI; the
        // transmute matches the `sqlite3_auto_extension` argument type. Called
        // exactly once, before any connection in this process is opened.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(),
            >(sqlite_vec::sqlite3_vec_init as *const ())));
        }
    });
}
```

- [ ] **Step 5: Run the gate test to verify it passes**

Run: `cargo test -p entheai-memory vec0_knn_roundtrip_gate`
Expected: PASS (`test result: ok. 1 passed`).

If it fails on the query form, try `ORDER BY distance LIMIT ?3` (drop `AND k = ?3`) — but the `AND k = ?` form is required once a partition-key filter is present, so prefer it. If the extension fails to load, confirm `sqlite-vec` and `rusqlite` compiled against the same bundled SQLite (both from this workspace's `Cargo.lock`).

- [ ] **Step 6: Confirm nothing else broke, then commit**

Run: `cargo test -p entheai-memory` → Expected: all existing tests still pass (24 + 1 new).
Run: `cargo clippy -p entheai-memory -- -D warnings` → Expected: no warnings.
Run: `cargo fmt -p entheai-memory`.

```bash
git add crates/memory/Cargo.toml crates/memory/src/store.rs Cargo.lock
git commit -m "feat(memory): register sqlite-vec extension + vec0 KNN gate test"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 1: `entries` → rowid table + in-place migration + `store_inner` seam

Give `entries` an integer `id` (the join key `vec0`/FTS5 need) and route all writes through one private `store_inner` that takes a precomputed embedding, so later tasks extend a single write path and tests can inject vectors without network.

**Files:**
- Modify: `crates/memory/src/store.rs`

- [ ] **Step 1: Write the failing migration test**

Add to `mod tests`:

```rust
#[tokio::test]
async fn migrates_pre_v1_without_rowid_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    // Hand-build a pre-v1 schema (WITHOUT ROWID, no `id`) with one row.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (
                 namespace TEXT NOT NULL, key TEXT NOT NULL, content TEXT NOT NULL,
                 metadata TEXT, embedding BLOB,
                 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                 PRIMARY KEY (namespace, key)
             ) WITHOUT ROWID;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entries VALUES ('learnings','k1','old content',NULL,NULL,100,100)",
            [],
        )
        .unwrap();
    }
    // Opening with the v1 store must migrate and preserve the row.
    let store = SqliteStore::open(&path, None).unwrap();
    let entry = store
        .get(Namespace::Learnings, "k1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.content, "old content");
    assert_eq!(entry.created_at, 100);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p entheai-memory migrates_pre_v1_without_rowid_table`
Expected: FAIL — the row is lost or the open errors (old schema has no `id`, new upsert/`RETURNING id` doesn't match).

- [ ] **Step 3: Replace the schema + open path**

In `crates/memory/src/store.rs`, replace the two `execute_batch` schema strings in `open` and `open_memory` with a call to a shared `ensure_schema`, and call `ensure_vec_extension()` first. Both constructors become:

```rust
pub fn open(path: impl AsRef<Path>, embedder: Option<Embedder>) -> Result<Self, MemoryError> {
    ensure_vec_extension();
    let conn = Connection::open_with_flags(
        path.as_ref(),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA mmap_size = 268435456;
         PRAGMA foreign_keys = ON;",
    )?;
    ensure_schema(&conn)?;
    Ok(SqliteStore { db: Arc::new(Mutex::new(conn)), embedder, recall: RecallParams::default() })
}

pub fn open_memory(embedder: Option<Embedder>) -> Result<Self, MemoryError> {
    ensure_vec_extension();
    let conn = Connection::open_in_memory()?;
    ensure_schema(&conn)?;
    Ok(SqliteStore { db: Arc::new(Mutex::new(conn)), embedder, recall: RecallParams::default() })
}
```

Add the `recall` field and a temporary `RecallParams` (fully defined in Task 5 — for now just enough to compile):

```rust
/// Recall scoring parameters (populated from config; see Task 5).
#[derive(Debug, Clone)]
pub struct RecallParams {
    pub w_recency: f64,
    pub w_conf: f64,
    pub half_life_days: f64,
    pub rrf_k: f64,
    pub overfetch: usize,
}

impl Default for RecallParams {
    fn default() -> Self {
        Self { w_recency: 0.3, w_conf: 0.2, half_life_days: 14.0, rrf_k: 60.0, overfetch: 3 }
    }
}
```

Add the `recall: RecallParams` field to the `struct SqliteStore` definition and a setter beside `set_embedder`:

```rust
pub fn set_recall_params(&mut self, params: RecallParams) {
    self.recall = params;
}
```

Add the schema/migration helpers (module-level functions):

```rust
fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![table],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Create the v1 schema, migrating a pre-v1 `WITHOUT ROWID` `entries` table
/// (no `id` column) in place. `vec_entries`/`entries_fts` are added in later
/// tasks; this task only establishes the rowid `entries` table + `meta`.
fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    if table_exists(conn, "entries")? && !column_exists(conn, "entries", "id")? {
        conn.execute_batch(
            "ALTER TABLE entries RENAME TO entries_old;
             CREATE TABLE entries (
                 id INTEGER PRIMARY KEY,
                 namespace TEXT NOT NULL, key TEXT NOT NULL, content TEXT NOT NULL,
                 metadata TEXT, embedding BLOB,
                 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                 UNIQUE (namespace, key));
             INSERT INTO entries
                 (namespace, key, content, metadata, embedding, created_at, updated_at)
                 SELECT namespace, key, content, metadata, embedding, created_at, updated_at
                 FROM entries_old;
             DROP TABLE entries_old;",
        )?;
    } else {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                 id INTEGER PRIMARY KEY,
                 namespace TEXT NOT NULL, key TEXT NOT NULL, content TEXT NOT NULL,
                 metadata TEXT, embedding BLOB,
                 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                 UNIQUE (namespace, key));",
        )?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_ns_created ON entries(namespace, created_at DESC);
         CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);",
    )?;
    Ok(())
}
```

- [ ] **Step 4: Route `store` through `store_inner`**

Replace the body of the `Memory::store` impl so it embeds (if an embedder is set), then delegates to `store_inner`:

```rust
async fn store(
    &self,
    namespace: Namespace,
    key: &str,
    content: &str,
    metadata: Option<serde_json::Value>,
) -> Result<Entry, MemoryError> {
    let embedding = match &self.embedder {
        Some(emb) => Some(emb.embed(content).await?),
        None => None,
    };
    self.store_inner(namespace, key, content, metadata, embedding).await
}
```

Add `store_inner` as an inherent method on `SqliteStore` (does all DB work; returns the real `created_at`). The `id` column is `INTEGER PRIMARY KEY` = the rowid; `RETURNING id, created_at` gives both. Later tasks add FTS/vec sync inside the blocking closure marked below.

```rust
async fn store_inner(
    &self,
    namespace: Namespace,
    key: &str,
    content: &str,
    metadata: Option<serde_json::Value>,
    embedding: Option<Vec<f32>>,
) -> Result<Entry, MemoryError> {
    let ns = namespace.as_str().to_string();
    let k = key.to_string();
    let c = content.to_string();
    let meta_json = metadata
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| MemoryError::Embedding(e.into()))?;
    let embedding_blob = embedding.as_ref().map(|v| f32_slice_to_blob(v));

    let db = Arc::clone(&self.db);
    let now = timestamp_ms();
    let (ns2, k2, c2) = (ns.clone(), k.clone(), c.clone());

    let created_at = tokio::task::spawn_blocking(move || -> rusqlite::Result<i64> {
        let conn = Self::lock_db(&db);
        let (_id, created_at): (i64, i64) = conn.query_row(
            "INSERT INTO entries (namespace, key, content, metadata, embedding, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (namespace, key) DO UPDATE SET
                 content = excluded.content, metadata = excluded.metadata,
                 embedding = excluded.embedding, updated_at = excluded.updated_at
             RETURNING id, created_at",
            params![ns2, k2, c2, meta_json, embedding_blob, now, now],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        // --- SYNC POINT: Task 2 adds FTS, Task 3 adds vec, keyed by `_id`. ---
        Ok(created_at)
    })
    .await
    .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;

    Ok(Entry { namespace, key: k, content: c, metadata, created_at, updated_at: now })
}
```

- [ ] **Step 5: Run the migration + existing store tests to verify they pass**

Run: `cargo test -p entheai-memory migrates_pre_v1_without_rowid_table store_and_get store_updates_existing on_disk_persistence`
Expected: PASS (migration preserves the row; `RETURNING` still yields the preserved `created_at`).

- [ ] **Step 6: Full crate gate + commit**

Run: `cargo test -p entheai-memory` → Expected: all pass.
Run: `cargo clippy -p entheai-memory -- -D warnings` → Expected: clean.
Run: `cargo fmt -p entheai-memory`.

```bash
git add crates/memory/src/store.rs
git commit -m "feat(memory): rowid entries table + pre-v1 migration + store_inner seam"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 2: FTS5 keyword index synced on write + delete + backfill

Add `entries_fts` (rusqlite bundled ships FTS5), keep it in sync inside `store_inner`/`delete`, and backfill it on open.

**Files:**
- Modify: `crates/memory/src/store.rs`

- [ ] **Step 1: Write the failing FTS test**

Add to `mod tests`:

```rust
#[tokio::test]
async fn fts_keyword_search_finds_content() {
    let store = SqliteStore::open_memory(None).unwrap();
    store.store(Namespace::Learnings, "k1", "prefer Arc<str> over String for shared config", None).await.unwrap();
    store.store(Namespace::Learnings, "k2", "the cargo test harness runs in parallel", None).await.unwrap();
    let ids = store.fts_ids(Namespace::Learnings, "cargo", 10).await.unwrap();
    assert_eq!(ids.len(), 1, "only k2 mentions cargo");
}

#[tokio::test]
async fn delete_removes_fts_row() {
    let store = SqliteStore::open_memory(None).unwrap();
    store.store(Namespace::Learnings, "k1", "unique-token-xyz here", None).await.unwrap();
    store.delete(Namespace::Learnings, "k1").await.unwrap();
    let ids = store.fts_ids(Namespace::Learnings, "unique-token-xyz", 10).await.unwrap();
    assert!(ids.is_empty());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-memory fts_keyword_search_finds_content delete_removes_fts_row`
Expected: FAIL — `no method named fts_ids`.

- [ ] **Step 3: Create the FTS table + backfill in `ensure_schema`**

Append to the `execute_batch` at the end of `ensure_schema` (before `Ok(())`):

```rust
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(content);",
    )?;
    // Backfill any entries missing an FTS row (fresh table, or migrated DB).
    conn.execute(
        "INSERT INTO entries_fts(rowid, content)
             SELECT e.id, e.content FROM entries e
             LEFT JOIN entries_fts f ON f.rowid = e.id
             WHERE f.rowid IS NULL",
        [],
    )?;
```

- [ ] **Step 4: Sync FTS in `store_inner` and `delete`**

At the `--- SYNC POINT ---` in `store_inner`'s blocking closure, add (using the `_id` bound from `RETURNING`; rename `_id` → `id`):

```rust
        // FTS: delete-then-insert keeps the keyword row in sync on upsert.
        conn.execute("DELETE FROM entries_fts WHERE rowid = ?1", params![id])?;
        conn.execute(
            "INSERT INTO entries_fts(rowid, content) VALUES (?1, ?2)",
            params![id, c2],
        )?;
```

Replace the `delete` impl body so it removes the FTS row (and, after Task 3, the vec row) before the entry:

```rust
async fn delete(&self, namespace: Namespace, key: &str) -> Result<(), MemoryError> {
    let ns = namespace.as_str().to_string();
    let k = key.to_string();
    let db = Arc::clone(&self.db);
    tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = Self::lock_db(&db);
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM entries WHERE namespace = ?1 AND key = ?2",
                params![ns, k],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            conn.execute("DELETE FROM entries_fts WHERE rowid = ?1", params![id])?;
            // Task 3 adds: DELETE FROM vec_entries WHERE rowid = ?1
            conn.execute("DELETE FROM entries WHERE id = ?1", params![id])?;
        }
        Ok(())
    })
    .await
    .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;
    Ok(())
}
```

Add the `fts_ids` helper (also used by `search_hybrid` in Task 5). It builds a safe MATCH string by OR-joining quoted alphanumeric tokens — arbitrary user text can't inject FTS5 syntax errors:

```rust
/// Build an FTS5 MATCH query from free text: quote each alphanumeric token and
/// OR-join. Returns None when the query has no usable tokens.
fn fts_match_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() { None } else { Some(terms.join(" OR ")) }
}

impl SqliteStore { /* add near store_inner */
    /// Namespace-scoped BM25 keyword search → entry ids, best match first.
    // `#[allow(dead_code)]` is temporary: only tests call this until Task 5's
    // `search_hybrid` consumes it. Task 5 removes the attribute.
    #[allow(dead_code)]
    async fn fts_ids(&self, namespace: Namespace, query: &str, limit: usize) -> Result<Vec<i64>, MemoryError> {
        let Some(match_q) = fts_match_query(query) else { return Ok(Vec::new()); };
        let ns = namespace.as_str().to_string();
        let db = Arc::clone(&self.db);
        let ids = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<i64>> {
            let conn = Self::lock_db(&db);
            let mut stmt = conn.prepare(
                "SELECT e.id FROM entries_fts
                 JOIN entries e ON e.id = entries_fts.rowid
                 WHERE entries_fts MATCH ?1 AND e.namespace = ?2
                 ORDER BY bm25(entries_fts) LIMIT ?3",
            )?;
            let ids = stmt
                .query_map(params![match_q, ns, limit as i64], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .await
        .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;
        Ok(ids)
    }
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p entheai-memory fts_keyword_search_finds_content delete_removes_fts_row delete_removes_entry`
Expected: PASS.

- [ ] **Step 6: Full crate gate + commit**

Run: `cargo test -p entheai-memory` → all pass.
Run: `cargo clippy -p entheai-memory -- -D warnings` → clean. Then `cargo fmt -p entheai-memory`.

```bash
git add crates/memory/src/store.rs
git commit -m "feat(memory): FTS5 keyword index synced on write/delete + backfill"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 3: lazy `vec_entries` ANN index synced on write/delete + backfill + KNN helper

Wire `sqlite-vec` into the write path (lazy DIM) and add a namespace-scoped KNN helper. This replaces the O(n) cosine scan's data source.

**Files:**
- Modify: `crates/memory/src/store.rs`

- [ ] **Step 1: Write the failing ANN + backfill tests**

Add to `mod tests`:

```rust
#[tokio::test]
async fn vec_knn_round_trip_via_store_inner() {
    let store = SqliteStore::open_memory(None).unwrap();
    // store_inner lets us inject embeddings with no network.
    store.store_inner(Namespace::Learnings, "a", "alpha", None, Some(vec![1.0, 0.0, 0.0, 0.0])).await.unwrap();
    store.store_inner(Namespace::Learnings, "b", "beta",  None, Some(vec![0.0, 1.0, 0.0, 0.0])).await.unwrap();
    store.store_inner(Namespace::Learnings, "c", "gamma", None, Some(vec![0.0, 0.0, 1.0, 0.0])).await.unwrap();
    let ids = store.vec_ids(Namespace::Learnings, &[0.9, 0.1, 0.0, 0.0], 2).await.unwrap();
    assert_eq!(ids.first().copied(), store.id_of(Namespace::Learnings, "a").await.unwrap());
}

#[tokio::test]
async fn vec_backfilled_from_existing_embeddings_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bf.db");
    {
        let store = SqliteStore::open(&path, None).unwrap();
        store.store_inner(Namespace::Tools, "t1", "output", None, Some(vec![0.1, 0.2, 0.3, 0.4])).await.unwrap();
    }
    // Reopen: ensure_schema must recreate vec_entries at the remembered DIM and
    // backfill it, so a KNN query still finds the row.
    let store = SqliteStore::open(&path, None).unwrap();
    let ids = store.vec_ids(Namespace::Tools, &[0.1, 0.2, 0.3, 0.4], 1).await.unwrap();
    assert_eq!(ids.len(), 1);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-memory vec_knn_round_trip_via_store_inner vec_backfilled_from_existing_embeddings_on_open`
Expected: FAIL — `no method named vec_ids` / `id_of`.

- [ ] **Step 3: Add the lazy vec-table helpers**

Add module-level helpers to `crates/memory/src/store.rs`:

```rust
fn meta_get_usize(conn: &Connection, k: &str) -> rusqlite::Result<Option<usize>> {
    let v: Option<String> = conn
        .query_row("SELECT v FROM meta WHERE k = ?1", params![k], |r| r.get(0))
        .optional()?;
    Ok(v.and_then(|s| s.parse::<usize>().ok()))
}

/// Create `vec_entries` sized to `dim` (idempotent), record the DIM in `meta`,
/// and backfill any entries whose embedding matches that DIM. Cosine distance
/// mirrors the pre-v1 cosine-similarity semantics.
fn ensure_vec_table(conn: &Connection, dim: usize) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_entries USING vec0(
             namespace text partition key,
             embedding float[{dim}] distance_metric=cosine);"
    ))?;
    conn.execute(
        "INSERT OR IGNORE INTO meta(k, v) VALUES ('embed_dim', ?1)",
        params![dim.to_string()],
    )?;
    conn.execute(
        "INSERT INTO vec_entries(rowid, namespace, embedding)
             SELECT e.id, e.namespace, e.embedding FROM entries e
             WHERE e.embedding IS NOT NULL AND length(e.embedding) = ?1
               AND e.id NOT IN (SELECT rowid FROM vec_entries)",
        params![(dim * 4) as i64],
    )?;
    Ok(())
}
```

In `ensure_schema`, after the `meta` table is created, recreate the vec table when a DIM is already remembered (so search/backfill work on reopen, before any write this session):

```rust
    if let Some(dim) = meta_get_usize(conn, "embed_dim")? {
        ensure_vec_table(conn, dim)?;
    }
```

- [ ] **Step 4: Sync vec in `store_inner` and `delete`**

Extend `store_inner`'s blocking closure at the SYNC POINT (after the FTS block) so it lazily creates the vec table and upserts the vector — skipping (with a log) on a DIM change:

```rust
        if let Some(ref emb) = embedding {
            match meta_get_usize(&conn, "embed_dim")? {
                None => ensure_vec_table(&conn, emb.len())?,
                Some(d) if d == emb.len() => {}
                Some(d) => log::warn!(
                    "memory: embedding dim {} != store dim {} — skipping vector index for {}/{}",
                    emb.len(), d, ns2, k2
                ),
            }
            if meta_get_usize(&conn, "embed_dim")? == Some(emb.len()) {
                let blob = f32_slice_to_blob(emb);
                conn.execute("DELETE FROM vec_entries WHERE rowid = ?1", params![id])?;
                conn.execute(
                    "INSERT INTO vec_entries(rowid, namespace, embedding) VALUES (?1, ?2, ?3)",
                    params![id, ns2, blob],
                )?;
            }
        }
```

Note: move `embedding` into the closure. Because the closure now uses `embedding`, `ns2`, `k2`, and `id`, ensure they are all captured (they already are). The `embedding_blob` local computed earlier in Task 1 is now redundant — the entry's `embedding` column is still written from it in the upsert `VALUES`, so keep `embedding_blob` for the `entries` row and use the moved `embedding: Option<Vec<f32>>` for the vec table. Keep both: `embedding_blob` (for `entries.embedding`) is derived before the closure; move `embedding` in for the vec insert.

In `delete`, enable the vec deletion line (uncomment the Task-3 placeholder):

```rust
            conn.execute("DELETE FROM vec_entries WHERE rowid = ?1", params![id])?;
```

Guard it so it doesn't error when `vec_entries` doesn't exist yet (no embeddings ever written). Use:

```rust
            if table_exists(&conn, "vec_entries")? {
                conn.execute("DELETE FROM vec_entries WHERE rowid = ?1", params![id])?;
            }
```

- [ ] **Step 5: Add the `vec_ids` + `id_of` helpers**

```rust
impl SqliteStore {
    /// Namespace-scoped ANN KNN → entry ids, nearest first. Empty if no vec
    /// table exists yet (no embeddings written).
    // Temporary — Task 5's `search_hybrid` consumes it and removes the attribute.
    #[allow(dead_code)]
    async fn vec_ids(&self, namespace: Namespace, query: &[f32], k: usize) -> Result<Vec<i64>, MemoryError> {
        let ns = namespace.as_str().to_string();
        let blob = f32_slice_to_blob(query);
        let db = Arc::clone(&self.db);
        let ids = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<i64>> {
            let conn = Self::lock_db(&db);
            if !table_exists(&conn, "vec_entries")? {
                return Ok(Vec::new());
            }
            let mut stmt = conn.prepare(
                "SELECT rowid FROM vec_entries
                 WHERE namespace = ?1 AND embedding MATCH ?2 AND k = ?3
                 ORDER BY distance",
            )?;
            let ids = stmt
                .query_map(params![ns, blob, k as i64], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .await
        .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;
        Ok(ids)
    }

    /// Test-only helper: the integer id for a namespace+key, if present.
    #[cfg(test)]
    async fn id_of(&self, namespace: Namespace, key: &str) -> Result<Option<i64>, MemoryError> {
        let ns = namespace.as_str().to_string();
        let k = key.to_string();
        let db = Arc::clone(&self.db);
        let id = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<i64>> {
            let conn = Self::lock_db(&db);
            conn.query_row(
                "SELECT id FROM entries WHERE namespace = ?1 AND key = ?2",
                params![ns, k],
                |r| r.get(0),
            )
            .optional()
        })
        .await
        .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;
        Ok(id)
    }
}
```

- [ ] **Step 6: Run to verify it passes**

Run: `cargo test -p entheai-memory vec_knn_round_trip_via_store_inner vec_backfilled_from_existing_embeddings_on_open`
Expected: PASS.

- [ ] **Step 7: Full crate gate + commit**

Run: `cargo test -p entheai-memory` → all pass. `cargo clippy -p entheai-memory -- -D warnings` → clean. `cargo fmt -p entheai-memory`.

```bash
git add crates/memory/src/store.rs
git commit -m "feat(memory): lazy sqlite-vec ANN index synced on write/delete + backfill"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 4: `recall.rs` — pure RRF fusion + recency decay + final-score math

Pure, DB-free, exhaustively unit-tested scoring functions (spec §2).

**Files:**
- Create: `crates/memory/src/recall.rs`
- Modify: `crates/memory/src/lib.rs`

- [ ] **Step 1: Write the failing tests (inside the new file)**

Create `crates/memory/src/recall.rs`:

```rust
//! Pure recall scoring: reciprocal-rank fusion of ranked id lists, recency
//! decay, and the final blended score. No DB or async — trivially testable.

use std::collections::HashMap;

/// Weights + constants for the final blended score (from config; see Task 5).
#[derive(Debug, Clone, Copy)]
pub struct ScoreWeights {
    pub w_recency: f64,
    pub w_conf: f64,
    pub half_life_days: f64,
    pub rrf_k: f64,
}

/// Reciprocal-rank fusion over several ranked id lists (each best-first):
/// `rrf(id) = Σ_i 1 / (k + rank_i)`, rank 1-based. Ids absent from a list
/// contribute nothing from it.
pub fn rrf(lists: &[&[i64]], k: f64) -> HashMap<i64, f64> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for list in lists {
        for (rank0, &id) in list.iter().enumerate() {
            *scores.entry(id).or_insert(0.0) += 1.0 / (k + (rank0 as f64 + 1.0));
        }
    }
    scores
}

/// Exponential recency term: `exp(-age_days / half_life_days)`.
/// `decay(now, now) == 1.0`; strictly decreasing in age.
pub fn recency_decay(created_at_ms: i64, now_ms: i64, half_life_days: f64) -> f64 {
    let age_days = ((now_ms - created_at_ms).max(0) as f64) / 86_400_000.0;
    (-age_days / half_life_days.max(f64::EPSILON)).exp()
}

/// `final = rrf + w_recency·recency + w_conf·confidence`.
pub fn final_score(rrf_score: f64, recency: f64, confidence: f64, w: &ScoreWeights) -> f64 {
    rrf_score + w.w_recency * recency + w.w_conf * confidence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_rewards_appearing_high_in_both_lists() {
        // id 7 is rank 1 in list A and rank 1 in list B → highest.
        let a = [7_i64, 3, 9];
        let b = [7_i64, 1, 2];
        let scores = rrf(&[&a, &b], 60.0);
        let top = scores.iter().max_by(|x, y| x.1.partial_cmp(y.1).unwrap()).unwrap();
        assert_eq!(*top.0, 7);
    }

    #[test]
    fn rrf_single_list_ranks_by_position() {
        let a = [10_i64, 20, 30];
        let s = rrf(&[&a], 60.0);
        assert!(s[&10] > s[&20] && s[&20] > s[&30]);
    }

    #[test]
    fn decay_is_one_at_zero_age_and_monotone() {
        let now = 1_000_000_000_000;
        assert!((recency_decay(now, now, 14.0) - 1.0).abs() < 1e-9);
        let day = 86_400_000_i64;
        let d1 = recency_decay(now - day, now, 14.0);
        let d10 = recency_decay(now - 10 * day, now, 14.0);
        assert!(d1 < 1.0 && d10 < d1);
    }

    #[test]
    fn final_score_blends_terms() {
        let w = ScoreWeights { w_recency: 0.3, w_conf: 0.2, half_life_days: 14.0, rrf_k: 60.0 };
        let s = final_score(0.5, 1.0, 0.5, &w);
        assert!((s - (0.5 + 0.3 * 1.0 + 0.2 * 0.5)).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: Wire the module + run the tests**

Add to `crates/memory/src/lib.rs` after the other `mod` lines:

```rust
pub mod recall;
```

And extend the store re-export line to also export `RecallParams`:

```rust
pub use store::{RecallParams, SqliteStore};
```

Run: `cargo test -p entheai-memory recall::` → Expected: 4 passed.

- [ ] **Step 3: Commit**

Run: `cargo clippy -p entheai-memory -- -D warnings` → clean. `cargo fmt -p entheai-memory`.

```bash
git add crates/memory/src/recall.rs crates/memory/src/lib.rs
git commit -m "feat(memory): pure recall module — RRF fusion, recency decay, final score"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 5: hybrid `search` — vector + FTS → RRF → score

Rewrite `SqliteStore::search` to fuse both arms with `recall.rs`. This is where the O(n) cosine scan is deleted.

**Files:**
- Modify: `crates/memory/src/store.rs`
- Modify: `crates/memory/src/lib.rs` (doc on `ScoredEntry.score`)

- [ ] **Step 1: Write the failing hybrid test**

Add to `mod tests`. A vector-only hit and a keyword-only hit must both surface:

```rust
#[tokio::test]
async fn hybrid_search_surfaces_vector_and_keyword_hits() {
    let store = SqliteStore::open_memory(None).unwrap();
    // "vecmatch" has an embedding near the query but no query keyword.
    store.store_inner(Namespace::Learnings, "vec", "vecmatch semantically near", None, Some(vec![1.0, 0.0, 0.0, 0.0])).await.unwrap();
    // "kwmatch" contains the literal query token but a far embedding.
    store.store_inner(Namespace::Learnings, "kw", "kwmatch contains cargo token", None, Some(vec![0.0, 0.0, 1.0, 0.0])).await.unwrap();
    // Unrelated filler.
    store.store_inner(Namespace::Learnings, "z", "unrelated filler text", None, Some(vec![0.0, 1.0, 0.0, 0.0])).await.unwrap();

    let results = store
        .search_hybrid(Namespace::Learnings, Some(&[0.95, 0.05, 0.0, 0.0]), "cargo", 3)
        .await
        .unwrap();
    let keys: Vec<&str> = results.iter().map(|s| s.entry.key.as_str()).collect();
    assert!(keys.contains(&"vec"), "vector arm surfaces 'vec'");
    assert!(keys.contains(&"kw"), "keyword arm surfaces 'kw'");
}

#[tokio::test]
async fn search_keyword_only_without_embedder() {
    let store = SqliteStore::open_memory(None).unwrap();
    store.store(Namespace::Learnings, "k", "the retry helper lives here", None).await.unwrap();
    // No embedder → public search must still return keyword results (no error).
    let results = store.search(Namespace::Learnings, "retry", 5).await.unwrap();
    assert_eq!(results.len(), 1);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-memory hybrid_search_surfaces_vector_and_keyword_hits search_keyword_only_without_embedder`
Expected: FAIL — `no method named search_hybrid`; and `search_without_embedder_returns_error` (the old test) will now be wrong.

- [ ] **Step 3: Implement `search_hybrid` + rewrite `search`**

Add `search_hybrid` and a row-loader to `SqliteStore`, and replace the `Memory::search` body:

```rust
async fn search(
    &self,
    namespace: Namespace,
    query: &str,
    limit: usize,
) -> Result<Vec<ScoredEntry>, MemoryError> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    // Embed the query only if an embedder is configured; otherwise keyword-only.
    let query_vec = match &self.embedder {
        Some(emb) => Some(emb.embed(query).await?),
        None => None,
    };
    self.search_hybrid(namespace, query_vec.as_deref(), query, limit).await
}
```

```rust
impl SqliteStore {
    /// Hybrid recall: vector KNN (if a query embedding is given) + FTS5 keyword,
    /// fused with RRF and scored by recency + confidence. Over-fetches each arm
    /// by `recall.overfetch` before fusing.
    async fn search_hybrid(
        &self,
        namespace: Namespace,
        query_embedding: Option<&[f32]>,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<ScoredEntry>, MemoryError> {
        let over = (limit * self.recall.overfetch).max(limit).max(1);

        let vec_list = match query_embedding {
            Some(q) => self.vec_ids(namespace, q, over).await?,
            None => Vec::new(),
        };
        let fts_list = self.fts_ids(namespace, query_text, over).await?;

        if vec_list.is_empty() && fts_list.is_empty() {
            return Ok(Vec::new());
        }

        let fused = crate::recall::rrf(&[&vec_list, &fts_list], self.recall.rrf_k);

        // Load candidate rows (content/metadata/created_at) in one pass.
        let ids: Vec<i64> = fused.keys().copied().collect();
        let rows = self.load_entries(namespace, &ids).await?;

        let now = timestamp_ms();
        let weights = crate::recall::ScoreWeights {
            w_recency: self.recall.w_recency,
            w_conf: self.recall.w_conf,
            half_life_days: self.recall.half_life_days,
            rrf_k: self.recall.rrf_k,
        };

        let mut scored: Vec<ScoredEntry> = rows
            .into_iter()
            .map(|(id, entry)| {
                let confidence = entry
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("confidence"))
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.5);
                let recency = crate::recall::recency_decay(entry.created_at, now, weights.half_life_days);
                let rrf_score = fused.get(&id).copied().unwrap_or(0.0);
                let final_s = crate::recall::final_score(rrf_score, recency, confidence, &weights);
                ScoredEntry { entry, score: final_s as f32 }
            })
            .collect();

        scored.sort_unstable_by(|a, b| {
            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }

    /// Load entries by id within a namespace, returning `(id, Entry)`.
    async fn load_entries(
        &self,
        namespace: Namespace,
        ids: &[i64],
    ) -> Result<Vec<(i64, Entry)>, MemoryError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ns = namespace.as_str().to_string();
        let ids: Vec<i64> = ids.to_vec();
        let db = Arc::clone(&self.db);
        #[allow(clippy::type_complexity)]
        let rows = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<(i64, String, String, Option<String>, i64, i64)>> {
            let conn = Self::lock_db(&db);
            let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT id, key, content, metadata, created_at, updated_at FROM entries
                 WHERE namespace = ? AND id IN ({placeholders})"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut binds: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(ids.len() + 1);
            binds.push(&ns);
            for id in &ids {
                binds.push(id);
            }
            let out = stmt
                .query_map(binds.as_slice(), |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(out)
        })
        .await
        .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;

        let mut out = Vec::with_capacity(rows.len());
        for (id, key, content, metadata_json, created_at, updated_at) in rows {
            let metadata = metadata_json
                .map(|m| serde_json::from_str(&m))
                .transpose()
                .map_err(|e| MemoryError::Embedding(e.into()))?;
            out.push((id, Entry { namespace, key, content, metadata, created_at, updated_at }));
        }
        Ok(out)
    }
}
```

Then **remove the temporary `#[allow(dead_code)]`** from `fts_ids` (Task 2) and `vec_ids` (Task 3) — `search_hybrid` now calls both, so they're live.

**Dead-code cleanup:** the O(n) cosine path is gone, so delete `cosine_similarity` and `blob_to_f32_vec` (both now unreferenced in non-test code → they'd trip clippy `-D warnings`) along with their tests — the three `cosine_*` tests and `blob_roundtrip`. **Keep** `f32_slice_to_blob` (used by `store_inner`, `vec_ids`, and the Task 0 gate test); the vec round-trip tests now cover it.

- [ ] **Step 4: Replace the obsolete embedder-error test**

The old `search_without_embedder_returns_error` test asserted an error; hybrid now returns keyword results instead. Replace its body with the keyword-only expectation (or delete it — `search_keyword_only_without_embedder` already covers this). Delete `search_without_embedder_returns_error`.

- [ ] **Step 5: Update the `ScoredEntry` doc**

In `crates/memory/src/lib.rs`, change the `ScoredEntry.score` doc comment from the cosine wording to:

```rust
    /// Blended relevance score (RRF + recency + confidence). Higher = more relevant.
    pub score: f32,
```

- [ ] **Step 6: Run to verify it passes**

Run: `cargo test -p entheai-memory hybrid_search_surfaces_vector_and_keyword_hits search_keyword_only_without_embedder`
Expected: PASS.

- [ ] **Step 7: Full crate gate + commit**

Run: `cargo test -p entheai-memory` → all pass (removed cosine tests no longer counted).
Run: `cargo clippy -p entheai-memory -- -D warnings` → clean (no dead-code warnings). `cargo fmt -p entheai-memory`.

```bash
git add crates/memory/src/store.rs crates/memory/src/lib.rs
git commit -m "feat(memory): hybrid search — vector + FTS5 fused by RRF, scored by recency/confidence"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 6: config — on-by-default + recall weights + global path

Flip `[memory] enabled` on and add the recall knobs + global DB path (spec §4).

**Files:**
- Modify: `crates/config/src/lib.rs`

- [ ] **Step 1: Update the failing defaults test**

Replace the `memory_config_defaults` test body:

```rust
#[test]
fn memory_config_defaults() {
    let cfg = Config::from_toml_str("").unwrap();
    assert!(cfg.memory.enabled, "memory is on by default in v1");
    assert_eq!(cfg.memory.path, "~/.cache/entheai/memory.db");
    assert!((cfg.memory.w_recency - 0.3).abs() < 1e-9);
    assert!((cfg.memory.half_life_days - 14.0).abs() < 1e-9);
    assert_eq!(cfg.memory.rrf_k, 60.0);
    assert_eq!(cfg.memory.recall_overfetch, 3);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-config memory_config_defaults`
Expected: FAIL — `enabled` is `false`, path differs, and the new fields don't exist.

- [ ] **Step 3: Implement the config changes**

In `MemoryConfig`, add the recall fields and change the `enabled` default. `serde(default)` on `bool` defaults to `false`, so add an explicit default fn to flip it on:

```rust
    #[serde(default = "default_memory_enabled")]
    pub enabled: bool,
```

Add the new fields to the struct:

```rust
    #[serde(default = "default_w_recency")]
    pub w_recency: f64,
    #[serde(default = "default_w_conf")]
    pub w_conf: f64,
    #[serde(default = "default_half_life_days")]
    pub half_life_days: f64,
    #[serde(default = "default_rrf_k")]
    pub rrf_k: f64,
    #[serde(default = "default_recall_overfetch")]
    pub recall_overfetch: usize,
```

Add the default fns and change the path default:

```rust
fn default_memory_enabled() -> bool { true }
fn default_memory_path() -> String { "~/.cache/entheai/memory.db".into() }
fn default_w_recency() -> f64 { 0.3 }
fn default_w_conf() -> f64 { 0.2 }
fn default_half_life_days() -> f64 { 14.0 }
fn default_rrf_k() -> f64 { 60.0 }
fn default_recall_overfetch() -> usize { 3 }
```

Update the `impl Default for MemoryConfig` to match (set `enabled: true`, the new path, and the five new fields to their default-fn values).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-config memory_config_defaults`
Expected: PASS.

- [ ] **Step 5: Full crate gate + commit**

Run: `cargo test -p entheai-config` → all pass. `cargo clippy -p entheai-config -- -D warnings` → clean. `cargo fmt -p entheai-config`.

```bash
git add crates/config/src/lib.rs
git commit -m "feat(config): memory on-by-default + recall weights + global cache path"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 7: bin wiring — construct the store + one-shot via `run_task_with_memory`

Build the shared store, optional embedder, runtime, and scope in `bin/entheai`, and route the one-shot path through `run_task_with_memory` (spec §4).

**Files:**
- Modify: `bin/entheai/src/main.rs`

- [ ] **Step 1: Add the construction helpers**

Add to `bin/entheai/src/main.rs` (module-level fns):

```rust
/// Expand a leading `~` to the user's home directory.
fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// Map the config's `[memory]` block to the runtime config.
fn memory_runtime_config(m: &entheai_config::MemoryConfig) -> entheai_memory::MemoryRuntimeConfig {
    entheai_memory::MemoryRuntimeConfig {
        enabled: m.enabled,
        strict: m.strict,
        retrieve_codebase: m.retrieve_codebase,
        retrieve_learnings: m.retrieve_learnings,
        retrieve_trajectories: m.retrieve_trajectories,
        max_context_chars: m.max_context_chars,
        tool_spill_chars: m.tool_spill_chars,
        evidence_tools: if m.evidence_tools.is_empty() {
            vec!["run_shell".into(), "search".into()]
        } else {
            m.evidence_tools.clone()
        },
    }
}

/// Build the shared memory store from config: an optional embedder (only when
/// `embed_provider` is configured — keeps on-by-default offline-safe) plus the
/// recall params. Returns `None` when memory is disabled.
fn build_memory(cfg: &Config) -> anyhow::Result<Option<entheai_memory::SharedMemory>> {
    if !cfg.memory.enabled {
        return Ok(None);
    }
    let embedder = cfg.memory.embed_provider.as_ref().and_then(|p| {
        cfg.providers.get(p).map(|pc| {
            entheai_memory::Embedder::new(pc.base_url.clone(), cfg.memory.embed_model.clone())
        })
    });
    let path = expand_home(&cfg.memory.path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut store = entheai_memory::SqliteStore::open(&path, embedder)?;
    store.set_recall_params(entheai_memory::RecallParams {
        w_recency: cfg.memory.w_recency,
        w_conf: cfg.memory.w_conf,
        half_life_days: cfg.memory.half_life_days,
        rrf_k: cfg.memory.rrf_k,
        overfetch: cfg.memory.recall_overfetch,
    });
    Ok(Some(std::sync::Arc::new(store)))
}
```

- [ ] **Step 2: Wire the one-shot path**

In `main`, after `let policy = …;`, construct the shared memory + a session id:

```rust
    let shared_memory = build_memory(&cfg)?;
    let session_id = uuid::Uuid::new_v4().to_string();
```

Replace the non-fanout one-shot branch (the `else` arm calling `agent.run_task(...)`) with a memory-aware run:

```rust
            } else {
                let mut prompter = entheai_permission::StdinPrompter;
                let mut messages = Vec::new();
                if let Some(sp) = &system_prompt {
                    messages.push(ChatMessage::system(sp.clone()));
                }
                messages.push(ChatMessage::user(prompt));
                let runtime = shared_memory
                    .clone()
                    .map(|m| entheai_memory::MemoryRuntime::new(m, memory_runtime_config(&cfg.memory)));
                let scope = entheai_memory::MemoryScope {
                    session_id: session_id.clone(),
                    task_id: "oneshot".to_string(),
                    cwd: root.clone(),
                    role: None,
                };
                let answer = agent
                    .run_task_with_memory(
                        messages, &registry, &policy, &mut prompter, None,
                        runtime.as_ref(), scope,
                    )
                    .await?;
                println!("{answer}");
            }
```

- [ ] **Step 3: Add the `entheai-memory` dependency to the bin**

`bin/entheai/Cargo.toml` almost certainly already depends on `entheai-memory` transitively via `entheai-core`, but the bin now names its types directly. Add it explicitly:

Run: `cargo add entheai-memory --path crates/memory -p entheai` (or add `entheai-memory = { path = "../../crates/memory" }` to `bin/entheai/Cargo.toml` matching the sibling-crate path style already used there).

- [ ] **Step 4: Verify it builds + a smoke test**

Run: `cargo build -p entheai`
Expected: compiles.

Run (offline smoke — memory on, no embedder, keyword-only, must not hang or error):
```bash
cat > /tmp/entheai-mem-smoke.toml <<'TOML'
default_model = "osaurus/qwen3-coder"
[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"
[memory]
path = "/tmp/entheai-smoke-memory.db"
TOML
rm -f /tmp/entheai-smoke-memory.db
cargo run -q -p entheai -- --config /tmp/entheai-mem-smoke.toml --no-companion "say hi" 2>&1 | tail -5 || true
```
Expected: it runs the agent path (a model error is fine if Osaurus is down); crucially it does **not** panic in memory construction and creates `/tmp/entheai-smoke-memory.db`. Verify: `test -f /tmp/entheai-smoke-memory.db && echo DB_CREATED`.

- [ ] **Step 5: Full gate + commit**

Run: `./scripts/check.sh`
Expected: fmt clean, clippy `-D warnings` clean, all tests pass.

```bash
git add bin/entheai/src/main.rs bin/entheai/Cargo.toml Cargo.lock
git commit -m "feat(bin): construct shared memory + wire one-shot via run_task_with_memory"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 8: `--memory <list|search|stats>` inspection mode

An operator command that runs before the agent path and exits (spec §5).

**Files:**
- Modify: `bin/entheai/src/main.rs`

- [ ] **Step 1: Add the CLI flag**

Add to `struct Cli`:

```rust
    /// Inspect memory then exit: `--memory stats`, `--memory list <namespace>`,
    /// `--memory search <namespace> <query...>`.
    #[arg(long = "memory", num_args = 1.., value_name = "ARGS")]
    memory: Vec<String>,
```

- [ ] **Step 2: Dispatch it early in `main`**

Immediately after `let root = …;` (memory inspection doesn't need the tool registry or companion), add:

```rust
    if !cli.memory.is_empty() {
        return run_memory_cmd(&cfg, &cli.memory).await;
    }
```

- [ ] **Step 3: Implement the command**

```rust
/// Inspect the memory store and exit. Namespaces: codebase, learnings,
/// trajectories, tools, subagents.
async fn run_memory_cmd(cfg: &Config, args: &[String]) -> anyhow::Result<()> {
    use entheai_memory::Namespace;
    let store = build_memory(cfg)?
        .ok_or_else(|| anyhow::anyhow!("memory is disabled ([memory] enabled = false)"))?;

    let parse_ns = |s: &str| -> anyhow::Result<Namespace> {
        s.parse::<Namespace>().map_err(|_| {
            anyhow::anyhow!("unknown namespace '{s}' (codebase|learnings|trajectories|tools|subagents)")
        })
    };

    match args.first().map(String::as_str) {
        Some("stats") => {
            let mut total = 0usize;
            for ns in [Namespace::Codebase, Namespace::Learnings, Namespace::Trajectories, Namespace::Tools, Namespace::Subagents] {
                let n = store.list(ns, usize::MAX, 0).await?.len();
                total += n;
                println!("{:<13} {n}", ns.as_str());
            }
            println!("{:<13} {total}", "total");
        }
        Some("list") => {
            let ns = parse_ns(args.get(1).map(String::as_str).unwrap_or(""))?;
            for e in store.list(ns, 20, 0).await? {
                let preview: String = e.content.chars().take(80).collect();
                println!("{}  {}  {}", e.key, e.created_at, preview.replace('\n', " "));
            }
        }
        Some("search") => {
            let ns = parse_ns(args.get(1).map(String::as_str).unwrap_or(""))?;
            let query = args.get(2..).map(|q| q.join(" ")).unwrap_or_default();
            if query.trim().is_empty() {
                anyhow::bail!("usage: --memory search <namespace> <query...>");
            }
            for s in store.search(ns, &query, 10).await? {
                let preview: String = s.entry.content.chars().take(80).collect();
                println!("[{:.3}] {}  {}", s.score, s.entry.key, preview.replace('\n', " "));
            }
        }
        _ => anyhow::bail!("usage: --memory <list <ns> | search <ns> <query...> | stats>"),
    }
    Ok(())
}
```

- [ ] **Step 4: Verify it works against the smoke DB**

```bash
cargo build -p entheai
# Seed one entry via the store, then inspect. Reuse the smoke config from Task 7.
cargo run -q -p entheai -- --config /tmp/entheai-mem-smoke.toml --memory stats
cargo run -q -p entheai -- --config /tmp/entheai-mem-smoke.toml --memory search learnings "hi"
```
Expected: `stats` prints per-namespace counts + total; `search` prints scored rows (possibly empty) and exits 0 — no agent run, no companion window.

- [ ] **Step 5: Full gate + commit**

Run: `./scripts/check.sh` → clean.

```bash
git add bin/entheai/src/main.rs
git commit -m "feat(bin): --memory list/search/stats inspection mode"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 9: TUI wiring — memory in the interactive run loop

Thread the shared runtime + scope into `entheai_tui::run` and swap its internal `run_task` for `run_task_with_memory` (spec §4).

**Files:**
- Modify: `crates/tui/src/lib.rs`
- Modify: `crates/tui/Cargo.toml`
- Modify: `bin/entheai/src/main.rs`

- [ ] **Step 1: Add the `entheai-memory` dep to the TUI**

Add to `crates/tui/Cargo.toml` `[dependencies]` (matching the sibling-path style already used for `entheai-core` etc.):

```toml
entheai-memory = { path = "../memory" }
```

- [ ] **Step 2: Extend `run` + `event_loop` signatures**

Add two parameters to `pub async fn run<P: Provider + 'static>(...)` (after `companion_tx`):

```rust
    memory: Option<std::sync::Arc<entheai_memory::MemoryRuntime>>,
    scope: entheai_memory::MemoryScope,
```

Thread both through the `event_loop(...)` call inside `run` (add them as the last two arguments) and add matching parameters to the `event_loop` function signature.

- [ ] **Step 3: Swap the agent call**

Find the agent invocation (currently around `crates/tui/src/lib.rs:433`):

```rust
.run_task(history, &registry, &policy, &mut prompter, Some(event_tx))
```

Before the block that moves `history` into the run (this call sits inside a spawned task / async block), clone the memory handle + scope so they can move in:

```rust
let mem = memory.clone();
let sc = scope.clone();
```

Replace the call with:

```rust
.run_task_with_memory(history, &registry, &policy, &mut prompter, Some(event_tx), mem.as_deref(), sc)
```

`Option<Arc<MemoryRuntime>>::as_deref()` yields `Option<&MemoryRuntime>`, which matches `run_task_with_memory`'s `memory` parameter. Read the surrounding spawn/move code and place the `mem`/`sc` clones so each iteration gets a fresh `scope` (bump `task_id` per submission if the loop reuses one scope — e.g. `MemoryScope { task_id: format!("turn-{turn_n}"), ..sc.clone() }` — otherwise every turn overwrites the same trajectory key). If the loop has no turn counter, a per-submission `uuid` for `task_id` is fine.

- [ ] **Step 4: Update the bin's TUI call**

In `bin/entheai/src/main.rs`, the `None =>` (interactive) arm calls `entheai_tui::run(...)`. Build the runtime + scope and pass them:

```rust
        None => {
            let companion_tx = companion.as_ref().map(|c| c.state_tx.clone());
            let runtime = shared_memory
                .clone()
                .map(|m| std::sync::Arc::new(entheai_memory::MemoryRuntime::new(m, memory_runtime_config(&cfg.memory))));
            let scope = entheai_memory::MemoryScope {
                session_id: session_id.clone(),
                task_id: "tui".to_string(),
                cwd: root.clone(),
                role: None,
            };
            entheai_tui::run(
                agent, registry, policy, model_id.clone(), cfg, root.clone(),
                cli.fanout, system_prompt, companion_tx, runtime, scope,
            )
            .await?;
        }
```

- [ ] **Step 5: Verify it builds**

Run: `cargo build -p entheai-tui -p entheai`
Expected: compiles. (The TUI is driven manually; no automated UI test here — its pure renderers are already covered by the TUI-flow plan.)

Run: `cargo test -p entheai-tui`
Expected: existing TUI tests still pass.

- [ ] **Step 6: Full gate + commit**

Run: `./scripts/check.sh` → clean.

```bash
git add crates/tui/src/lib.rs crates/tui/Cargo.toml bin/entheai/src/main.rs Cargo.lock
git commit -m "feat(tui): thread shared memory into the interactive run loop"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 10: fan-out sub-agents read + write the shared store

Give fan-out leaves a `MemoryRuntime` built from the same `SharedMemory`, so sub-agents read collective memory and write their trajectories/learnings (spec §4). Writes stay safe — the store serializes them behind its single `Arc<Mutex<Connection>>` + `spawn_blocking`.

**Files:**
- Modify: `crates/orchestrator/src/lib.rs`
- Modify: `crates/orchestrator/Cargo.toml`
- Modify: `bin/entheai/src/main.rs`

- [ ] **Step 1: Write the failing integration test**

Add a test module at the bottom of `crates/orchestrator/src/lib.rs` (or extend an existing one). It verifies a sub-agent run **writes** to the shared store. Use the read-only path with a fake/loopback model is heavy; instead test the seam directly by asserting the per-leaf runtime config is built and a store handed to a leaf is written. Keep it a focused unit test on a small helper:

```rust
#[cfg(test)]
mod memory_wiring_tests {
    use super::*;
    use entheai_memory::{Memory, MemoryRuntime, MemoryScope, Namespace, SqliteStore};
    use std::sync::Arc;

    #[tokio::test]
    async fn subagent_runtime_writes_trajectory_to_shared_store() {
        let store: entheai_memory::SharedMemory = Arc::new(SqliteStore::open_memory(None).unwrap());
        let rt = MemoryRuntime::new(Arc::clone(&store), runtime_config_enabled());
        let scope = MemoryScope {
            session_id: "fanout-test".into(),
            task_id: "coder-0".into(),
            cwd: std::path::PathBuf::from("/tmp"),
            role: Some("backend".into()),
        };
        rt.record_final_answer(&scope, "test/model", "did the thing", &[])
            .await
            .unwrap();
        let trajectories = store.list(Namespace::Trajectories, 10, 0).await.unwrap();
        assert_eq!(trajectories.len(), 1, "the sub-agent wrote its trajectory");
    }

    fn runtime_config_enabled() -> entheai_memory::MemoryRuntimeConfig {
        entheai_memory::MemoryRuntimeConfig { enabled: true, ..Default::default() }
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-orchestrator subagent_runtime_writes_trajectory_to_shared_store`
Expected: FAIL to compile — `entheai-memory` isn't a dependency of the orchestrator yet.

- [ ] **Step 3: Add the dependency + a runtime-config helper**

Add to `crates/orchestrator/Cargo.toml` `[dependencies]`:

```toml
entheai-memory = { path = "../memory" }
```

Add a helper near the top of `crates/orchestrator/src/lib.rs` (mirrors the bin's mapping — kept local to avoid a cross-crate coupling):

```rust
/// Map config `[memory]` → runtime config for fan-out sub-agents.
fn fanout_memory_config(cfg: &Config) -> entheai_memory::MemoryRuntimeConfig {
    entheai_memory::MemoryRuntimeConfig {
        enabled: cfg.memory.enabled,
        strict: cfg.memory.strict,
        retrieve_codebase: cfg.memory.retrieve_codebase,
        retrieve_learnings: cfg.memory.retrieve_learnings,
        retrieve_trajectories: cfg.memory.retrieve_trajectories,
        max_context_chars: cfg.memory.max_context_chars,
        tool_spill_chars: cfg.memory.tool_spill_chars,
        evidence_tools: if cfg.memory.evidence_tools.is_empty() {
            vec!["run_shell".into(), "search".into()]
        } else {
            cfg.memory.evidence_tools.clone()
        },
    }
}
```

- [ ] **Step 4: Thread `memory` through the fan-out functions**

Add a `memory: Option<entheai_memory::SharedMemory>` parameter to the public entry point and every leaf. Signatures become:

```rust
pub async fn run_fanout(
    config: &Config,
    root: &Path,
    task: &str,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
    memory: Option<entheai_memory::SharedMemory>,
) -> anyhow::Result<String>

async fn run_fanout_readonly(
    config: &Config, root: &Path, task: &str, memory: Option<entheai_memory::SharedMemory>,
) -> anyhow::Result<String>

async fn run_subagent(
    config: &Config, root: &Path, st: SubTask,
    memory: Option<entheai_memory::SharedMemory>, session: String,
) -> SubResult

async fn run_coder(
    config: &Config, wt: worktree::Worktree, st: SubTask,
    events: Option<tokio::sync::mpsc::UnboundedSender<FanoutEvent>>,
    memory: Option<entheai_memory::SharedMemory>, session: String,
) -> CoderRun
```

In `run_fanout`, pass `memory.clone()` into `run_fanout_readonly` (fallback branch) and generate/reuse the `session` string (it already builds `let session = uuid…` for worktrees — reuse it, and generate one earlier for the fallback). In the `stream::iter(...).map(...)` that spawns coders, capture `let mem = memory.clone(); let session = session.clone();` and pass them into `run_coder`.

In `run_fanout_readonly`, generate `let session = uuid::Uuid::new_v4().simple().to_string();` before the fan-out and pass `memory.clone()` + `session.clone()` into each `run_subagent`. Because `run_subagent` now needs an index for a unique `task_id`, switch its `stream::iter` to `.enumerate()`:

```rust
    let results: Vec<SubResult> = stream::iter(subtasks.into_iter().enumerate())
        .map(|(i, st)| run_subagent(config, root, st, memory.clone(), format!("{session}-{i}")))
        .buffer_unordered(max_par)
        .collect()
        .await;
```

- [ ] **Step 5: Use `run_task_with_memory` in each leaf**

In `run_subagent`, build a runtime + scope and swap the call:

```rust
        let runtime = memory
            .as_ref()
            .map(|m| entheai_memory::MemoryRuntime::new(m.clone(), fanout_memory_config(config)));
        let scope = entheai_memory::MemoryScope {
            session_id: session.clone(),
            task_id: format!("sub-{}", st.role),
            cwd: root.to_path_buf(),
            role: Some(st.role.clone()),
        };
        let out = agent
            .run_task_with_memory(
                subagent_messages(&st.role, &st.task),
                &registry, &policy, &mut prompter, None,
                runtime.as_ref(), scope,
            )
            .await?;
```

In `run_coder`, do the same, scoping to the worktree:

```rust
        let runtime = memory
            .as_ref()
            .map(|m| entheai_memory::MemoryRuntime::new(m.clone(), fanout_memory_config(config)));
        let scope = entheai_memory::MemoryScope {
            session_id: session.clone(),
            task_id: format!("coder-{}", wt.index),
            cwd: wt.path.clone(),
            role: Some(st.role.clone()),
        };
        let out = agent
            .run_task_with_memory(
                coder_messages(&st.role, &st.task),
                &registry, &policy, &mut prompter, None,
                runtime.as_ref(), scope,
            )
            .await?;
```

(The orchestrator meta-calls — `orchestrate_once` for decompose/synthesis — stay on plain `run_task`; only leaves get memory.)

- [ ] **Step 6: Update the bin's fan-out call**

In `bin/entheai/src/main.rs`, the `if cli.fanout { … run_fanout(&cfg, &root, &prompt, None) … }` call gains the shared memory:

```rust
                let answer = entheai_orchestrator::run_fanout(
                    &cfg, &root, &prompt, None, shared_memory.clone(),
                ).await?;
```

Check the TUI's internal fan-out trigger too: if `crates/tui` calls `run_fanout`, add the `memory` argument there (pass the `Option<Arc<MemoryRuntime>>`'s underlying `SharedMemory` — thread the `SharedMemory` into the TUI as well if needed, or pass `None` for the TUI fan-out path in v1 and note it). Grep first: `grep -rn "run_fanout" crates/tui/src`. If found, pass `None` for now and add a `// TODO(memory-v1.1): thread shared store into TUI fan-out` comment to keep scope bounded; the one-shot `--fanout` path (the common case) is fully wired.

- [ ] **Step 7: Run to verify it passes**

Run: `cargo test -p entheai-orchestrator subagent_runtime_writes_trajectory_to_shared_store`
Expected: PASS.

Run: `cargo build -p entheai-orchestrator -p entheai-tui -p entheai`
Expected: compiles.

- [ ] **Step 8: Full gate + commit**

Run: `./scripts/check.sh` → clean.

```bash
git add crates/orchestrator/src/lib.rs crates/orchestrator/Cargo.toml bin/entheai/src/main.rs crates/tui/src/lib.rs Cargo.lock
git commit -m "feat(orchestrator): fan-out sub-agents read + write the shared memory store"
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Final verification

- [ ] **Workspace gate**

Run: `./scripts/check.sh`
Expected: fmt clean, clippy `-D warnings` clean, **all** workspace tests pass.

- [ ] **Success-criteria smoke (spec §8)**

```bash
# ANN + hybrid recall exercised by unit tests (Tasks 3, 5).
cargo test -p entheai-memory
# On-by-default, offline-safe: memory built, keyword recall, no hang.
cargo run -q -p entheai -- --config /tmp/entheai-mem-smoke.toml --memory stats
```
Expected: memory tests green; `--memory stats` prints per-namespace counts. If Osaurus (or another embed provider) is configured and running, `entheai --memory search learnings "cargo test"` returns relevant stored learnings.

- [ ] **Confirm preserved fixes**

Verify these previously-landed correctness fixes survived the store rewrite (spec §8): char-safe preview + task-scoped keys + `warn!` diagnostics live in `runtime.rs` (untouched); mutex-poison recovery (`lock_db`), `created_at` preservation (`RETURNING created_at`), and the 30s embedder timeout (`embed.rs`) live in the rewritten `store.rs`/`embed.rs` — grep to confirm `lock_db`, `RETURNING id, created_at`, and `truncate_str` are all still present.

---

## Notes for the executor

- **DIM reconciliation:** the spec's literal "default 1024" is realized as runtime-derived DIM (spec §1 also says "DIM comes from the embedder"). No `embed_dim` config field exists — this is intentional and removes a config-mismatch failure mode.
- **On-by-default is offline-safe** only because the embedder is optional; never make embedding mandatory in the store path.
- **Every commit pushes immediately** and uses scoped `git add`. This is the shared multi-session `main`.
- If `sqlite-vec`'s KNN SQL differs from what's written here, the **Task 0 gate test** is the source of truth — make it pass first, then mirror its exact query shape in Tasks 3 and 5.
