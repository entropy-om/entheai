use std::path::Path;
use std::sync::{Arc, Mutex, Once};

use rusqlite::{params, Connection, OpenFlags};

use super::{Embedder, Entry, Memory, MemoryError, Namespace, ScoredEntry};

/// Register the `sqlite-vec` loadable extension **process-globally** via the
/// FFI `sqlite3_auto_extension`. Idempotent (guarded by `Once`).
///
/// Contract and caveats — read before relying on this:
/// - `sqlite3_auto_extension` registers `sqlite3_vec_init` as an auto-extension
///   for the entire process. It only affects connections opened *after* it
///   runs; a `Connection` opened earlier will not have `vec0` available.
/// - It is **not** synchronized against a concurrent `Connection::open` on
///   another thread. `Once` guarantees the registration body runs at most once —
///   it does *not* establish a happens-before against opens elsewhere. Callers
///   that need `vec0` must call this before opening their own connection (each
///   `SqliteStore` constructor is expected to do exactly that, before its
///   `open`) and must not race a first registration against an open on another
///   thread.
// Called by every `SqliteStore` constructor (`open`/`open_memory`) before the
// connection is opened, so `vec0` is available to the tables that later tasks
// add.
fn ensure_vec_extension() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: the transmute reinterprets `sqlite_vec::sqlite3_vec_init`
        // (a function pointer, hence pointer-sized) as the exact SQLite
        // extension-entry ABI that `sqlite3_auto_extension` expects:
        // `unsafe extern "C" fn(*mut sqlite3, *mut *mut c_char,
        // *const sqlite3_api_routines) -> c_int`. sqlite-vec's own bindgen
        // `sqlite3`/`sqlite3_api_routines` types are ABI-identical to rusqlite's,
        // so the reinterpretation is sound; source and target are both
        // pointer-sized, satisfying `transmute`'s size requirement. This says
        // nothing about *when* it runs — see the fn-level ordering caveats.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut std::os::raw::c_char,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> std::os::raw::c_int,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

/// SQLite-backed [`Memory`] implementation.
///
/// Wraps a single `rusqlite::Connection` behind an `Arc<Mutex<>>`. All DB I/O
/// is dispatched through `spawn_blocking` so the async runtime never stalls,
/// even on single-threaded executors. WAL journal mode, 256 MB mmap, and
/// NORMAL synchronous are applied at open time.
///
/// Mutex poisoning is recovered via `into_inner()` — a panic in one DB
/// operation does not permanently brick the store. `spawn_blocking` panics
/// are mapped to `MemoryError::Internal`.
pub struct SqliteStore {
    db: Arc<Mutex<Connection>>,
    embedder: Option<Embedder>,
    recall: RecallParams,
}

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
        Self {
            w_recency: 0.3,
            w_conf: 0.2,
            half_life_days: 14.0,
            rrf_k: 60.0,
            overfetch: 3,
        }
    }
}

impl SqliteStore {
    /// Open (or create) the database at `path`, applying the schema and pragmas.
    pub fn open(path: impl AsRef<Path>, embedder: Option<Embedder>) -> Result<Self, MemoryError> {
        ensure_vec_extension();
        let mut conn = Connection::open_with_flags(
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
        ensure_schema(&mut conn)?;
        Ok(SqliteStore {
            db: Arc::new(Mutex::new(conn)),
            embedder,
            recall: RecallParams::default(),
        })
    }

    /// Open an in-memory database (for testing).
    pub fn open_memory(embedder: Option<Embedder>) -> Result<Self, MemoryError> {
        ensure_vec_extension();
        let mut conn = Connection::open_in_memory()?;
        ensure_schema(&mut conn)?;
        Ok(SqliteStore {
            db: Arc::new(Mutex::new(conn)),
            embedder,
            recall: RecallParams::default(),
        })
    }

    /// Set the embedder after construction.
    pub fn set_embedder(&mut self, embedder: Embedder) {
        self.embedder = Some(embedder);
    }

    /// Set the recall scoring parameters after construction.
    pub fn set_recall_params(&mut self, params: RecallParams) {
        self.recall = params;
    }

    /// Lock the connection, recovering from a poisoned mutex.
    fn lock_db(db: &Mutex<Connection>) -> std::sync::MutexGuard<'_, Connection> {
        db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Store (or upsert) an entry with a precomputed embedding.
    ///
    /// The single write seam behind [`Memory::store`]: the public method embeds
    /// (when an embedder is configured) and delegates here. `created_at` is
    /// preserved on conflict via `RETURNING`.
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
            let mut conn = Self::lock_db(&db);
            let tx = conn.transaction()?;
            let (id, created_at): (i64, i64) = tx.query_row(
                "INSERT INTO entries (namespace, key, content, metadata, embedding, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (namespace, key) DO UPDATE SET
                     content = excluded.content, metadata = excluded.metadata,
                     embedding = excluded.embedding, updated_at = excluded.updated_at
                 RETURNING id, created_at",
                params![ns2, k2, c2, meta_json, embedding_blob, now, now],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            // --- SYNC POINT: FTS + vec, inside the SAME tx. ---
            // FTS: delete-then-insert keeps the keyword row in sync on upsert.
            // The whole upsert + FTS sync commits atomically (RAII rollback on any `?`).
            tx.execute("DELETE FROM entries_fts WHERE rowid = ?1", params![id])?;
            tx.execute(
                "INSERT INTO entries_fts(rowid, content) VALUES (?1, ?2)",
                params![id, c2],
            )?;
            // Vec (ANN): lazily create the vec table at the first embedding's DIM,
            // then keep it in sync. Mismatched dims are skipped (logged) so a model
            // change can't poison the write path.
            if let Some(ref emb) = embedding {
                let dim_ok = match meta_get_usize(&tx, "embed_dim")? {
                    None => {
                        ensure_vec_table(&tx, emb.len())?;
                        true
                    }
                    Some(d) if d == emb.len() => true,
                    Some(d) => {
                        log::warn!(
                            "memory: embedding dim {} != store dim {} — skipping vector index for {}/{}",
                            emb.len(),
                            d,
                            ns2,
                            k2
                        );
                        false
                    }
                };
                // Always drop this id's existing/backfilled vec row first, THEN
                // re-insert only when dims match. Unconditional delete clears a stale
                // vector when the embedding dim changed, and avoids a duplicate-rowid
                // error when ensure_vec_table's backfill just inserted this row on the
                // first write.
                if table_exists(&tx, "vec_entries")? {
                    tx.execute("DELETE FROM vec_entries WHERE rowid = ?1", params![id])?;
                }
                if dim_ok {
                    let blob = f32_slice_to_blob(emb);
                    tx.execute(
                        "INSERT INTO vec_entries(rowid, namespace, embedding) VALUES (?1, ?2, ?3)",
                        params![id, ns2, blob],
                    )?;
                }
            }
            tx.commit()?;
            Ok(created_at)
        })
        .await
        .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;

        Ok(Entry {
            namespace,
            key: k,
            content: c,
            metadata,
            created_at,
            updated_at: now,
        })
    }
}

/// Build an FTS5 MATCH query from free text: quote each alphanumeric token and
/// OR-join. Returns None when the query has no usable tokens.
fn fts_match_query(query: &str) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .filter(|t| seen.insert(t.to_lowercase())) // dedup, case-insensitive
        .take(32) // cap OR-chain length
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

impl SqliteStore {
    /// Namespace-scoped BM25 keyword search → entry ids, best match first.
    // Temporary — Task 5's `search_hybrid` consumes it and removes the attribute.
    #[allow(dead_code)]
    async fn fts_ids(
        &self,
        namespace: Namespace,
        query: &str,
        limit: usize,
    ) -> Result<Vec<i64>, MemoryError> {
        let Some(match_q) = fts_match_query(query) else {
            return Ok(Vec::new());
        };
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

impl SqliteStore {
    /// Namespace-scoped ANN KNN → entry ids, nearest first. Empty if no vec
    /// table exists yet (no embeddings written).
    // Temporary — Task 5's `search_hybrid` consumes it and removes the attribute.
    #[allow(dead_code)]
    async fn vec_ids(
        &self,
        namespace: Namespace,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<i64>, MemoryError> {
        let ns = namespace.as_str().to_string();
        let blob = f32_slice_to_blob(query);
        let query_len = query.len();
        let db = Arc::clone(&self.db);
        let ids = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<i64>> {
            let conn = Self::lock_db(&db);
            if !table_exists(&conn, "vec_entries")? {
                return Ok(Vec::new());
            }
            // Query dim must match the index dim, else vec0's MATCH errors.
            // Degrade to keyword-only (empty) on mismatch rather than hard-fail.
            if meta_get_usize(&conn, "embed_dim")? != Some(query_len) {
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

#[async_trait::async_trait]
impl Memory for SqliteStore {
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
        self.store_inner(namespace, key, content, metadata, embedding)
            .await
    }

    async fn get(&self, namespace: Namespace, key: &str) -> Result<Option<Entry>, MemoryError> {
        let ns = namespace.as_str().to_string();
        let k = key.to_string();
        let db = Arc::clone(&self.db);

        let ns2 = ns.clone();
        let k2 = k.clone();
        let row = tokio::task::spawn_blocking(move || {
            let conn = Self::lock_db(&db);
            conn.query_row(
                "SELECT content, metadata, created_at, updated_at FROM entries WHERE namespace = ?1 AND key = ?2",
                params![ns2, k2],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            ).optional()
        })
        .await
        .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;

        match row {
            Some((content, metadata_json, created_at, updated_at)) => {
                let metadata = metadata_json
                    .map(|m| serde_json::from_str(&m))
                    .transpose()
                    .map_err(|e| MemoryError::Embedding(e.into()))?;
                Ok(Some(Entry {
                    namespace,
                    key: k,
                    content,
                    metadata,
                    created_at,
                    updated_at,
                }))
            }
            None => Ok(None),
        }
    }

    async fn search(
        &self,
        namespace: Namespace,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ScoredEntry>, MemoryError> {
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| MemoryError::Embedding(anyhow::anyhow!("no embedder configured")))?;

        let query_vec = embedder.embed(query).await?;
        let ns = namespace.as_str().to_string();
        let db = Arc::clone(&self.db);

        let rows = tokio::task::spawn_blocking(move || {
            let conn = Self::lock_db(&db);
            let mut stmt = conn.prepare(
                "SELECT key, content, metadata, embedding, created_at, updated_at
                 FROM entries
                 WHERE namespace = ?1 AND embedding IS NOT NULL",
            )?;
            let mut rows = Vec::new();
            let mut q = stmt.query(params![ns])?;
            while let Some(row) = q.next()? {
                let emb: Vec<u8> = row.get(3)?;
                rows.push((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    emb,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ));
            }
            Ok::<_, rusqlite::Error>(rows)
        })
        .await
        .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;

        let mut scored: Vec<ScoredEntry> = Vec::with_capacity(rows.len());
        for (key, content, metadata_json, emb_blob, created_at, updated_at) in rows {
            let emb = blob_to_f32_vec(&emb_blob);
            if emb.len() != query_vec.len() {
                // Dimension mismatch — embedding model changed. Skip this entry
                // rather than failing the entire search (v0.1 best-effort).
                continue;
            }
            let score = cosine_similarity(&query_vec, &emb);
            let metadata = metadata_json
                .map(|m| serde_json::from_str(&m))
                .transpose()
                .map_err(|e| MemoryError::Embedding(e.into()))?;
            scored.push(ScoredEntry {
                entry: Entry {
                    namespace,
                    key,
                    content,
                    metadata,
                    created_at,
                    updated_at,
                },
                score,
            });
        }

        scored.sort_unstable_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }

    async fn delete(&self, namespace: Namespace, key: &str) -> Result<(), MemoryError> {
        let ns = namespace.as_str().to_string();
        let k = key.to_string();
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
            let mut conn = Self::lock_db(&db);
            let tx = conn.transaction()?;
            let id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM entries WHERE namespace = ?1 AND key = ?2",
                    params![ns, k],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = id {
                tx.execute("DELETE FROM entries_fts WHERE rowid = ?1", params![id])?;
                if table_exists(&tx, "vec_entries")? {
                    tx.execute("DELETE FROM vec_entries WHERE rowid = ?1", params![id])?;
                }
                tx.execute("DELETE FROM entries WHERE id = ?1", params![id])?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;
        Ok(())
    }

    async fn list(
        &self,
        namespace: Namespace,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Entry>, MemoryError> {
        let ns = namespace.as_str().to_string();
        let db = Arc::clone(&self.db);

        let rows = tokio::task::spawn_blocking(move || {
            let conn = Self::lock_db(&db);
            let mut stmt = conn.prepare(
                "SELECT key, content, metadata, created_at, updated_at
                 FROM entries
                 WHERE namespace = ?1
                 ORDER BY created_at DESC
                 LIMIT ?2 OFFSET ?3",
            )?;
            let rows: Vec<(String, String, Option<String>, i64, i64)> = stmt
                .query_map(params![ns, limit as i64, offset as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, rusqlite::Error>(rows)
        })
        .await
        .map_err(|e| MemoryError::Internal(format!("spawn_blocking panicked: {e}")))??;

        let mut entries = Vec::with_capacity(rows.len());
        for (key, content, metadata_json, created_at, updated_at) in rows {
            let metadata: Option<serde_json::Value> = metadata_json
                .map(|m| serde_json::from_str(&m))
                .transpose()
                .map_err(|e| MemoryError::Embedding(e.into()))?;
            entries.push(Entry {
                namespace,
                key,
                content,
                metadata,
                created_at,
                updated_at,
            });
        }

        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info(?1) WHERE name = ?2",
        params![table, column],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn table_exists(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![table],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

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

/// Create the v1 schema, migrating a pre-v1 `WITHOUT ROWID` `entries` table
/// (no `id` column) in place. `vec_entries`/`entries_fts` are added in later
/// tasks; this task only establishes the rowid `entries` table + `meta`.
fn ensure_schema(conn: &mut Connection) -> rusqlite::Result<()> {
    if table_exists(conn, "entries")? && !column_exists(conn, "entries", "id")? {
        // Atomic rebuild: IMMEDIATE tx acquires the write lock up front (serializing
        // concurrent openers on this shared checkout) and rolls back via RAII on any
        // early `?`, so a mid-migration failure leaves the original WITHOUT ROWID
        // table intact rather than stranding rows in `entries_old`.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(
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
        tx.commit()?;
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
    conn.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(content);")?;
    // Backfill any entries missing an FTS row (fresh table, or migrated DB).
    conn.execute(
        "INSERT INTO entries_fts(rowid, content)
             SELECT e.id, e.content FROM entries e
             LEFT JOIN entries_fts f ON f.rowid = e.id
             WHERE f.rowid IS NULL",
        [],
    )?;
    // Recreate the vec table when a DIM is already remembered, so KNN/backfill
    // work on reopen before any write this session (fresh DBs have no DIM yet).
    //
    // A pre-v1 DB with embeddings but no recorded `embed_dim` gets no vec table
    // here (DIM unknown) — it self-heals on the next embedding write. Pre-v1 memory
    // was off-by-default, so such DBs are effectively nonexistent.
    if let Some(dim) = meta_get_usize(conn, "embed_dim")? {
        ensure_vec_table(conn, dim)?;
    }
    Ok(())
}

fn timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Cosine similarity between two equal-length f32 vectors.
#[inline]
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let (dot, na, nb) = a
        .iter()
        .zip(b.iter())
        .fold((0.0f32, 0.0f32, 0.0f32), |(d, na, nb), (&x, &y)| {
            (x.mul_add(y, d), x.mul_add(x, na), y.mul_add(y, nb))
        });
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Serialize `&[f32]` to a blob of little-endian bytes.
fn f32_slice_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Deserialize a blob back to `Vec<f32>`.
fn blob_to_f32_vec(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Extension trait: map `QueryReturnedNoRows` to `Ok(None)`.
trait QueryRowExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> QueryRowExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        let s = cosine_similarity(&v, &v);
        assert!((s - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let s = cosine_similarity(&a, &b);
        assert!(s.abs() < 1e-6);
    }

    #[test]
    fn cosine_opposite_is_minus_one() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let s = cosine_similarity(&a, &b);
        assert!((s + 1.0).abs() < 1e-6);
    }

    #[test]
    fn blob_roundtrip() {
        let v = vec![1.0f32, -2.5, 0.0, 7.77];
        let blob = f32_slice_to_blob(&v);
        assert_eq!(blob.len(), 16);
        let back = blob_to_f32_vec(&blob);
        assert_eq!(v.len(), back.len());
        for (a, b) in v.iter().zip(back.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[tokio::test]
    async fn store_and_get() {
        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(Namespace::Learnings, "tip/1", "use Arc<str>", None)
            .await
            .unwrap();

        let entry = store
            .get(Namespace::Learnings, "tip/1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.content, "use Arc<str>");
    }

    #[tokio::test]
    async fn store_updates_existing() {
        let store = SqliteStore::open_memory(None).unwrap();
        let first = store
            .store(Namespace::Tools, "out", "v1", None)
            .await
            .unwrap();
        // Small sleep so timestamps differ.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let second = store
            .store(Namespace::Tools, "out", "v2", None)
            .await
            .unwrap();
        let entry = store.get(Namespace::Tools, "out").await.unwrap().unwrap();
        assert_eq!(entry.content, "v2");
        // created_at preserved, updated_at bumped.
        assert_eq!(second.created_at, first.created_at);
        assert!(second.updated_at > first.updated_at);
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let store = SqliteStore::open_memory(None).unwrap();
        let entry = store.get(Namespace::Tools, "nope").await.unwrap();
        assert!(entry.is_none());
    }

    #[tokio::test]
    async fn delete_removes_entry() {
        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(Namespace::Subagents, "s/1", "data", None)
            .await
            .unwrap();
        store.delete(Namespace::Subagents, "s/1").await.unwrap();
        assert!(store
            .get(Namespace::Subagents, "s/1")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn fts_keyword_search_finds_content() {
        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(
                Namespace::Learnings,
                "k1",
                "prefer Arc<str> over String for shared config",
                None,
            )
            .await
            .unwrap();
        store
            .store(
                Namespace::Learnings,
                "k2",
                "the cargo test harness runs in parallel",
                None,
            )
            .await
            .unwrap();
        let ids = store
            .fts_ids(Namespace::Learnings, "cargo", 10)
            .await
            .unwrap();
        assert_eq!(ids.len(), 1, "only k2 mentions cargo");
    }

    #[tokio::test]
    async fn delete_removes_fts_row() {
        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(Namespace::Learnings, "k1", "unique-token-xyz here", None)
            .await
            .unwrap();
        store.delete(Namespace::Learnings, "k1").await.unwrap();
        let ids = store
            .fts_ids(Namespace::Learnings, "unique-token-xyz", 10)
            .await
            .unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn fts_upsert_resyncs_content() {
        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(Namespace::Learnings, "k", "alphatoken original", None)
            .await
            .unwrap();
        store
            .store(Namespace::Learnings, "k", "betatoken replacement", None)
            .await
            .unwrap();
        assert!(
            store
                .fts_ids(Namespace::Learnings, "alphatoken", 10)
                .await
                .unwrap()
                .is_empty(),
            "old content no longer matches after upsert"
        );
        assert_eq!(
            store
                .fts_ids(Namespace::Learnings, "betatoken", 10)
                .await
                .unwrap()
                .len(),
            1,
            "new content matches"
        );
    }

    #[tokio::test]
    async fn fts_ids_is_namespace_scoped() {
        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(Namespace::Learnings, "k", "sharedterm here", None)
            .await
            .unwrap();
        store
            .store(Namespace::Tools, "k", "sharedterm here", None)
            .await
            .unwrap();
        assert_eq!(
            store
                .fts_ids(Namespace::Learnings, "sharedterm", 10)
                .await
                .unwrap()
                .len(),
            1,
            "only the learnings row is returned"
        );
    }

    #[tokio::test]
    async fn list_respects_offset_and_limit() {
        let store = SqliteStore::open_memory(None).unwrap();
        for i in 0..5 {
            store
                .store(
                    Namespace::Learnings,
                    &format!("k{i}"),
                    &format!("v{i}"),
                    None,
                )
                .await
                .unwrap();
        }
        let all = store.list(Namespace::Learnings, 10, 0).await.unwrap();
        assert_eq!(all.len(), 5);
        let page = store.list(Namespace::Learnings, 2, 2).await.unwrap();
        assert_eq!(page.len(), 2);
    }

    #[tokio::test]
    async fn namespaces_are_isolated() {
        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(Namespace::Tools, "a", "tool-a", None)
            .await
            .unwrap();
        store
            .store(Namespace::Learnings, "a", "learn-a", None)
            .await
            .unwrap();
        let t = store.get(Namespace::Tools, "a").await.unwrap().unwrap();
        let l = store.get(Namespace::Learnings, "a").await.unwrap().unwrap();
        assert_eq!(t.content, "tool-a");
        assert_eq!(l.content, "learn-a");
    }

    #[tokio::test]
    async fn namespace_parse_roundtrip() {
        for ns in [
            Namespace::Codebase,
            Namespace::Learnings,
            Namespace::Trajectories,
            Namespace::Tools,
            Namespace::Subagents,
        ] {
            let s = ns.as_str();
            let parsed: Namespace = s.parse().unwrap();
            assert_eq!(parsed, ns);
        }
    }

    #[test]
    fn invalid_namespace_rejects() {
        let err = "fridge".parse::<Namespace>().unwrap_err();
        assert!(matches!(err, MemoryError::InvalidNamespace(_)));
    }

    #[tokio::test]
    async fn search_without_embedder_returns_error() {
        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(Namespace::Learnings, "x", "data", None)
            .await
            .unwrap();
        let err = store
            .search(Namespace::Learnings, "query", 3)
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::Embedding(_)));
    }

    #[tokio::test]
    async fn metadata_roundtrip() {
        let store = SqliteStore::open_memory(None).unwrap();
        let meta = serde_json::json!({"priority": 1, "tags": ["rust", "sqlite"]});
        store
            .store(
                Namespace::Learnings,
                "meta/1",
                "content",
                Some(meta.clone()),
            )
            .await
            .unwrap();
        let entry = store
            .get(Namespace::Learnings, "meta/1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.metadata, Some(meta));
    }

    #[tokio::test]
    async fn on_disk_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");

        // Write in one store instance.
        {
            let store = SqliteStore::open(&path, None).unwrap();
            store
                .store(Namespace::Tools, "disk/1", "persisted", None)
                .await
                .unwrap();
        }

        // Read back in a fresh instance.
        {
            let store = SqliteStore::open(&path, None).unwrap();
            let entry = store
                .get(Namespace::Tools, "disk/1")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(entry.content, "persisted");
        }
    }

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

        // Prove the migration actually ran: the rowid schema now has an `id` column
        // the pre-v1 WITHOUT ROWID table did not. (get/store alone can't distinguish
        // migrated vs. not — both work on the old schema — so assert on `id` directly.)
        let raw = rusqlite::Connection::open(&path).unwrap();
        let id_cols: i64 = raw
            .query_row(
                "SELECT count(*) FROM pragma_table_info('entries') WHERE name = 'id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            id_cols, 1,
            "entries migrated to a rowid schema with an id column"
        );
    }

    #[test]
    fn vec0_knn_roundtrip_gate() {
        // Registers sqlite-vec, then proves the vec0 KNN query is a *real*
        // nearest-neighbour search over the little-endian f32 BLOB representation
        // production uses. The fixture is built to be hostile to two silent
        // failure modes:
        //   (a) a broken vec0 that returns rows in rowid / insertion order, and
        //   (b) a `namespace` partition filter that is silently a no-op.
        ensure_vec_extension();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE v USING vec0(
                 namespace text partition key,
                 embedding float[4] distance_metric=cosine);",
        )
        .unwrap();

        // Query lies on the z-axis. Within `learnings`, row 3 is nearest and
        // row 1 second — note row 3 is NOT the first-inserted rowid, so an
        // insertion-order fallback would give the wrong answer. Row 4 is the
        // global nearest (identical to the query) but lives in `tools`, so only
        // a correctly-applied partition filter keeps it out of `learnings`
        // results.
        let rows: [(i64, &str, [f32; 4]); 4] = [
            (1, "learnings", [0.0, 0.0, 0.5, 1.0]), // 2nd-nearest in learnings
            (2, "learnings", [0.0, 1.0, 0.0, 0.0]), // farthest in learnings
            (3, "learnings", [0.0, 0.1, 1.0, 0.0]), // nearest in learnings
            (4, "tools", [0.0, 0.0, 1.0, 0.0]),     // global nearest, wrong namespace
        ];
        for (id, ns, vec) in rows {
            conn.execute(
                "INSERT INTO v(rowid, namespace, embedding) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, ns, f32_slice_to_blob(&vec)],
            )
            .unwrap();
        }

        let query = f32_slice_to_blob(&[0.0, 0.0, 1.0, 0.0]);

        // (1) Nearest within `learnings` is row 3 — a non-first rowid, and NOT
        // the geometrically-closer row 4 that sits in the `tools` partition.
        let nearest: i64 = conn
            .query_row(
                "SELECT rowid FROM v
                 WHERE namespace = ?1 AND embedding MATCH ?2 AND k = ?3
                 ORDER BY distance",
                rusqlite::params!["learnings", &query, 1_i64],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            nearest, 3,
            "learnings nearest must be row 3 (non-first rowid), not insertion-order \
             row 1 nor the cross-namespace row 4"
        );

        // (2) Top-2 within `learnings`, ordered by distance, is [3, 1] — distance
        // actually drives the ordering (rowid order would yield [1, 2]).
        let mut stmt = conn
            .prepare(
                "SELECT rowid FROM v
                 WHERE namespace = ?1 AND embedding MATCH ?2 AND k = ?3
                 ORDER BY distance",
            )
            .unwrap();
        let top2: Vec<i64> = stmt
            .query_map(rusqlite::params!["learnings", &query, 2_i64], |row| {
                row.get(0)
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            top2,
            vec![3, 1],
            "distance ordering must yield [3, 1], not rowid order [1, 2]"
        );

        // (3) The same query in the `tools` partition returns row 4 — proving the
        // `namespace` filter is a genuine discriminator, not a no-op.
        let tools_nearest: i64 = conn
            .query_row(
                "SELECT rowid FROM v
                 WHERE namespace = ?1 AND embedding MATCH ?2 AND k = ?3
                 ORDER BY distance",
                rusqlite::params!["tools", &query, 1_i64],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            tools_nearest, 4,
            "tools partition must select row 4, confirming the namespace filter discriminates"
        );
    }

    #[tokio::test]
    async fn vec_knn_round_trip_via_store_inner() {
        let store = SqliteStore::open_memory(None).unwrap();
        // store_inner lets us inject embeddings with no network.
        store
            .store_inner(
                Namespace::Learnings,
                "a",
                "alpha",
                None,
                Some(vec![1.0, 0.0, 0.0, 0.0]),
            )
            .await
            .unwrap();
        store
            .store_inner(
                Namespace::Learnings,
                "b",
                "beta",
                None,
                Some(vec![0.0, 1.0, 0.0, 0.0]),
            )
            .await
            .unwrap();
        store
            .store_inner(
                Namespace::Learnings,
                "c",
                "gamma",
                None,
                Some(vec![0.0, 0.0, 1.0, 0.0]),
            )
            .await
            .unwrap();
        let ids = store
            .vec_ids(Namespace::Learnings, &[0.9, 0.1, 0.0, 0.0], 2)
            .await
            .unwrap();
        assert_eq!(
            ids.first().copied(),
            store.id_of(Namespace::Learnings, "a").await.unwrap()
        );
    }

    #[tokio::test]
    async fn vec_backfilled_from_existing_embeddings_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bf.db");
        {
            let store = SqliteStore::open(&path, None).unwrap();
            store
                .store_inner(
                    Namespace::Tools,
                    "t1",
                    "output",
                    None,
                    Some(vec![0.1, 0.2, 0.3, 0.4]),
                )
                .await
                .unwrap();
        }
        // Drop just the ANN index, leaving entries.embedding + meta.embed_dim. Reopen
        // must recreate vec_entries at the remembered DIM and backfill it from entries.
        {
            let raw = rusqlite::Connection::open(&path).unwrap();
            raw.execute_batch("DROP TABLE vec_entries;").unwrap();
        }
        let store = SqliteStore::open(&path, None).unwrap();
        let ids = store
            .vec_ids(Namespace::Tools, &[0.1, 0.2, 0.3, 0.4], 1)
            .await
            .unwrap();
        assert_eq!(
            ids.len(),
            1,
            "reopen rebuilt + backfilled vec_entries from entries.embedding"
        );
    }

    #[tokio::test]
    async fn vec_ids_empty_on_dim_mismatch_query() {
        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store_inner(
                Namespace::Learnings,
                "a",
                "alpha",
                None,
                Some(vec![1.0, 0.0, 0.0, 0.0]),
            )
            .await
            .unwrap();
        let ids = store
            .vec_ids(Namespace::Learnings, &[1.0, 0.0], 5)
            .await
            .unwrap(); // wrong dim
        assert!(ids.is_empty());
    }
}
