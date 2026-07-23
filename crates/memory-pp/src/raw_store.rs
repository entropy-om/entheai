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
    /// Raw failure traceback from empirical execution (a build, clippy check,
    /// or test that failed inside a fan-out worktree) — roadmap Phase 3.1:
    /// "knowledge grows in the soil. Even the brutal notes of failure."
    Trajectory,
    /// Current-world knowledge from external live sources (Valyu search,
    /// WorldMonitor events) — the brain knowing things AS THEY ARE.
    External,
    // Slice 2/3: CodebaseSnapshot, ObsidianNote
}

impl RawKind {
    fn as_str(self) -> &'static str {
        match self {
            RawKind::Transcript => "transcript",
            RawKind::ToolOutput => "tool_output",
            RawKind::Trajectory => "trajectory",
            RawKind::External => "external",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "transcript" => Some(RawKind::Transcript),
            "tool_output" => Some(RawKind::ToolOutput),
            "trajectory" => Some(RawKind::Trajectory),
            "external" => Some(RawKind::External),
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

/// Cheap to clone: every handle shares the one `Arc<Mutex<Connection>>`, so a
/// clone sees the same rows (used by the Slice-2 sidecar mesh to fetch previews).
#[derive(Clone)]
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

impl RawStore {
    pub fn open(path: &Path) -> Result<Self, PpError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        ensure_schema(&conn)?;
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_memory() -> Result<Self, PpError> {
        let conn = Connection::open_in_memory()?;
        ensure_schema(&conn)?;
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
        })
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
            let mut stmt =
                conn.prepare("SELECT id, kind, bytes, meta, created_at FROM raw WHERE id = ?1")?;
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
            let n = tx.execute(
                "DELETE FROM raw WHERE created_at < ?1",
                rusqlite::params![cutoff],
            )?;
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

    /// Most recently ingested spans, newest first, ≤ `k` — the anchors a
    /// checkpoint freeze records (roadmap 1.1). Ties on `created_at` break by
    /// id for a deterministic order.
    pub async fn recent(&self, k: usize) -> Result<Vec<RawSpan>, PpError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<RawSpan>, PpError> {
            let conn = db.lock().map_err(|_| PpError::Lock)?;
            let mut stmt = conn.prepare(
                "SELECT id, kind, created_at FROM raw
                 ORDER BY created_at DESC, id ASC
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(rusqlite::params![k as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for r in rows {
                let (id, kind_s, created_at) = r?;
                if let Some(kind) = RawKind::parse(&kind_s) {
                    out.push(RawSpan {
                        id,
                        kind,
                        score: 0.0,
                        created_at,
                    });
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| PpError::Join(e.to_string()))?
    }

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
                    out.push(RawSpan {
                        id,
                        kind,
                        score: (-bm) as f32,
                        created_at,
                    });
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| PpError::Join(e.to_string()))?
    }
}

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
        let a = s
            .ingest(RawKind::Transcript, "same content", None)
            .await
            .unwrap();
        let b = s
            .ingest(RawKind::Transcript, "same content", None)
            .await
            .unwrap();
        assert_eq!(a, b, "content-addressed id is stable");
        assert_eq!(s.count().await.unwrap(), 1, "re-ingest is a no-op");
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let s = RawStore::open_memory().unwrap();
        assert!(s.get("blake3:deadbeef").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn clone_shares_the_same_store() {
        // Slice 2: the sidecar mesh client holds a cheap handle to the store so it
        // can fetch candidate preview text — the clone must see the SAME rows.
        let s = RawStore::open_memory().unwrap();
        let handle = s.clone();
        let id = s
            .ingest(RawKind::Transcript, "shared row", None)
            .await
            .unwrap();
        let got = handle
            .get(&id)
            .await
            .unwrap()
            .expect("clone sees the ingest");
        assert_eq!(got.bytes, "shared row");
    }

    #[tokio::test]
    async fn prune_respects_retention() {
        let s = RawStore::open_memory().unwrap();
        s.ingest(RawKind::Transcript, "keep-me", None)
            .await
            .unwrap();
        assert_eq!(s.prune(90).await.unwrap(), 0, "recent row retained");
        assert_eq!(s.count().await.unwrap(), 1);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        assert_eq!(
            s.prune(0).await.unwrap(),
            1,
            "cutoff=now drops older-than-now"
        );
        assert_eq!(s.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn recall_finds_by_keyword_and_rehydrates() {
        let s = RawStore::open_memory().unwrap();
        s.ingest(RawKind::Transcript, "the auth login flow", None)
            .await
            .unwrap();
        s.ingest(RawKind::ToolOutput, "unrelated disk usage report", None)
            .await
            .unwrap();
        let spans = s.recall("auth", 10).await.unwrap();
        assert_eq!(spans.len(), 1, "only the matching span");
        assert_eq!(spans[0].kind, RawKind::Transcript);
        let rc = s.get(&spans[0].id).await.unwrap().unwrap();
        assert_eq!(
            rc.bytes, "the auth login flow",
            "span id rehydrates raw payload"
        );
    }

    #[tokio::test]
    async fn recall_respects_k() {
        let s = RawStore::open_memory().unwrap();
        for i in 0..5 {
            s.ingest(RawKind::Transcript, &format!("auth event number {i}"), None)
                .await
                .unwrap();
        }
        assert_eq!(s.recall("auth", 3).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn recall_punctuated_query_does_not_error() {
        let s = RawStore::open_memory().unwrap();
        s.ingest(RawKind::Transcript, "fix the auth bug in v2", None)
            .await
            .unwrap();
        // A raw FTS5 MATCH of this string is a syntax error (quotes, parens, `?`);
        // the sanitizer must turn it into a valid, recall-preserving query. Without
        // this, PP would silently never fire for punctuated prompts.
        let spans = s.recall("fix the \"auth\" bug (v2)?", 10).await.unwrap();
        assert!(!spans.is_empty(), "punctuated prompt still recalls");
    }

    #[test]
    fn sanitize_rejects_empty_and_quotes_tokens() {
        assert_eq!(sanitize_fts5_query("   "), None);
        assert_eq!(
            sanitize_fts5_query("a-b c"),
            Some("\"a\" OR \"b\" OR \"c\"".to_string())
        );
    }
}
