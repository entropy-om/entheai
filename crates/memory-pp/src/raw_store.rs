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
