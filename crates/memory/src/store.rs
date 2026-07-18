use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OpenFlags};

use super::{Embedder, Entry, Memory, MemoryError, Namespace, ScoredEntry};

/// SQLite-backed [`Memory`] implementation.
///
/// Wraps a single `rusqlite::Connection` behind an `Arc<Mutex<>>`. All DB I/O
/// is dispatched through `spawn_blocking` so the async runtime never stalls,
/// even on single-threaded executors. WAL journal mode, 256 MB mmap, and
/// NORMAL synchronous are applied at open time.
pub struct SqliteStore {
    db: Arc<Mutex<Connection>>,
    embedder: Option<Embedder>,
}

impl SqliteStore {
    /// Open (or create) the database at `path`, applying the schema and pragmas.
    pub fn open(path: impl AsRef<Path>, embedder: Option<Embedder>) -> Result<Self, MemoryError> {
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
             PRAGMA foreign_keys = ON;

             CREATE TABLE IF NOT EXISTS entries (
                 namespace TEXT NOT NULL,
                 key       TEXT NOT NULL,
                 content   TEXT NOT NULL,
                 metadata  TEXT,
                 embedding BLOB,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY (namespace, key)
             ) WITHOUT ROWID;

             CREATE INDEX IF NOT EXISTS idx_ns_created
                 ON entries(namespace, created_at DESC);",
        )?;

        Ok(SqliteStore {
            db: Arc::new(Mutex::new(conn)),
            embedder,
        })
    }

    /// Open an in-memory database (for testing).
    pub fn open_memory(embedder: Option<Embedder>) -> Result<Self, MemoryError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                 namespace TEXT NOT NULL,
                 key       TEXT NOT NULL,
                 content   TEXT NOT NULL,
                 metadata  TEXT,
                 embedding BLOB,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY (namespace, key)
             ) WITHOUT ROWID;

             CREATE INDEX IF NOT EXISTS idx_ns_created
                 ON entries(namespace, created_at DESC);",
        )?;

        Ok(SqliteStore {
            db: Arc::new(Mutex::new(conn)),
            embedder,
        })
    }

    /// Set the embedder after construction.
    pub fn set_embedder(&mut self, embedder: Embedder) {
        self.embedder = Some(embedder);
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
        let ns = namespace.as_str().to_string();
        let k = key.to_string();
        let c = content.to_string();
        let meta_json = metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| MemoryError::Embedding(e.into()))?;

        let embedding_blob = match &self.embedder {
            Some(emb) => {
                let vec = emb.embed(&c).await?;
                Some(f32_slice_to_blob(&vec))
            }
            None => None,
        };

        let db = Arc::clone(&self.db);
        let now = timestamp_ms();
        let ns2 = ns.clone();
        let k2 = k.clone();
        let c2 = c.clone();

        tokio::task::spawn_blocking(move || {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO entries (namespace, key, content, metadata, embedding, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (namespace, key) DO UPDATE SET
                     content = excluded.content,
                     metadata = excluded.metadata,
                     embedding = excluded.embedding,
                     updated_at = excluded.updated_at",
                params![ns2, k2, c2, meta_json, embedding_blob, now, now],
            )
        })
        .await
        .expect("spawn_blocking panicked")?;

        Ok(Entry {
            namespace,
            key: k,
            content: c,
            metadata,
            created_at: now,
            updated_at: now,
        })
    }

    async fn get(&self, namespace: Namespace, key: &str) -> Result<Option<Entry>, MemoryError> {
        let ns = namespace.as_str().to_string();
        let k = key.to_string();
        let db = Arc::clone(&self.db);

        let ns2 = ns.clone();
        let k2 = k.clone();
        let row = tokio::task::spawn_blocking(move || {
            let conn = db.lock().unwrap();
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
        .expect("spawn_blocking panicked")?;

        match row {
            Some((content, metadata_json, created_at, updated_at)) => {
                let metadata = metadata_json
                    .and_then(|m| serde_json::from_str(&m).ok());
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
            let conn = db.lock().unwrap();
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
        .expect("spawn_blocking panicked")?;

        let mut scored: Vec<ScoredEntry> = Vec::with_capacity(rows.len());
        for (key, content, metadata_json, emb_blob, created_at, updated_at) in rows {
            let emb = blob_to_f32_vec(&emb_blob);
            if emb.len() != query_vec.len() {
                continue;
            }
            let score = cosine_similarity(&query_vec, &emb);
            let metadata = metadata_json
                .and_then(|m| serde_json::from_str(&m).ok());
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

        scored.sort_unstable_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored)
    }

    async fn delete(&self, namespace: Namespace, key: &str) -> Result<(), MemoryError> {
        let ns = namespace.as_str().to_string();
        let k = key.to_string();
        let db = Arc::clone(&self.db);

        tokio::task::spawn_blocking(move || {
            let conn = db.lock().unwrap();
            conn.execute(
                "DELETE FROM entries WHERE namespace = ?1 AND key = ?2",
                params![ns, k],
            )
        })
        .await
        .expect("spawn_blocking panicked")?;

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
            let conn = db.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT key, content, metadata, created_at, updated_at
                 FROM entries
                 WHERE namespace = ?1
                 ORDER BY created_at DESC
                 LIMIT ?2 OFFSET ?3",
            )?;
            let rows = stmt
                .query_map(params![ns, limit as i64, offset as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();
            Ok::<_, rusqlite::Error>(rows)
        })
        .await
        .expect("spawn_blocking panicked")?;

        let entries = rows
            .into_iter()
            .map(|(key, content, metadata_json, created_at, updated_at)| {
                let metadata = metadata_json
                    .and_then(|m| serde_json::from_str(&m).ok());
                Entry {
                    namespace,
                    key,
                    content,
                    metadata,
                    created_at,
                    updated_at,
                }
            })
            .collect();

        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

        let entry = store.get(Namespace::Learnings, "tip/1").await.unwrap().unwrap();
        assert_eq!(entry.content, "use Arc<str>");
    }

    #[tokio::test]
    async fn store_updates_existing() {
        let store = SqliteStore::open_memory(None).unwrap();
        store
            .store(Namespace::Tools, "out", "v1", None)
            .await
            .unwrap();
        store
            .store(Namespace::Tools, "out", "v2", None)
            .await
            .unwrap();
        let entry = store.get(Namespace::Tools, "out").await.unwrap().unwrap();
        assert_eq!(entry.content, "v2");
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
        assert!(store.get(Namespace::Subagents, "s/1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_respects_offset_and_limit() {
        let store = SqliteStore::open_memory(None).unwrap();
        for i in 0..5 {
            store
                .store(Namespace::Learnings, &format!("k{i}"), &format!("v{i}"), None)
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
}
