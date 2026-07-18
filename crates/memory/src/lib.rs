//! Two-tier, five-namespace persistent memory for the agent.
//!
//! The [`Memory`] trait is the single store interface. Every call names a
//! namespace (one of the five [`Namespace`] variants) and the engine routes
//! accordingly:
//!
//! | Namespace   | Tier      | Storage                  |
//! |-------------|-----------|--------------------------|
//! | `Codebase`  | long-term | MCP (federated)          |
//! | `Learnings` | long-term | local SQLite + vector    |
//! | `Trajectories` | long-term | local SQLite + vector |
//! | `Tools`     | working   | local SQLite + vector    |
//! | `Subagents` | working   | local SQLite + vector    |
//!
//! v0.1: `Codebase` stored locally (MCP federation deferred). Vector search
//! uses brute-force cosine similarity (flat index); HNSW auto-promotion comes
//! in v0.2 when the dataset crosses ~5k vectors. Embeddings are obtained from
//! an OpenAI-compatible endpoint (Osaurus by default).

mod embed;
mod store;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use embed::Embedder;
pub use store::SqliteStore;

/// The five memory namespaces defined by the design spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Namespace {
    /// Repository structure: symbols, call graph, architecture, ADRs.
    Codebase,
    /// Durable facts, preferences, "how we solved X".
    Learnings,
    /// Reasoning paths, outcomes, scores (ReasoningBank).
    Trajectories,
    /// Tool results; large outputs spilled and recalled.
    Tools,
    /// Per-sub-agent scratch and outputs.
    Subagents,
}

impl Namespace {
    pub fn as_str(self) -> &'static str {
        match self {
            Namespace::Codebase => "codebase",
            Namespace::Learnings => "learnings",
            Namespace::Trajectories => "trajectories",
            Namespace::Tools => "tools",
            Namespace::Subagents => "subagents",
        }
    }
}

impl std::str::FromStr for Namespace {
    type Err = MemoryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "codebase" => Ok(Namespace::Codebase),
            "learnings" => Ok(Namespace::Learnings),
            "trajectories" => Ok(Namespace::Trajectories),
            "tools" => Ok(Namespace::Tools),
            "subagents" => Ok(Namespace::Subagents),
            other => Err(MemoryError::InvalidNamespace(other.to_string())),
        }
    }
}

/// A stored entry with its content, metadata, and optional embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub namespace: Namespace,
    pub key: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// An entry returned from a vector search, with its similarity score.
#[derive(Debug, Clone)]
pub struct ScoredEntry {
    pub entry: Entry,
    /// Cosine similarity in `[-1, 1]`. Higher = more relevant.
    pub score: f32,
}

/// Errors the memory layer can produce.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("unknown namespace: {0}")]
    InvalidNamespace(String),
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("embedding error: {0}")]
    Embedding(#[from] anyhow::Error),
    #[error("entry not found: {namespace}/{key}")]
    NotFound { namespace: String, key: String },
    #[error("internal error: {0}")]
    Internal(String),
}

/// The unified memory interface — two tiers, five namespaces, one engine.
#[async_trait]
pub trait Memory: Send + Sync {
    /// Store (or update) an entry. Automatically embeds `content` before
    /// writing if an embedder is configured.
    async fn store(
        &self,
        namespace: Namespace,
        key: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Entry, MemoryError>;

    /// Retrieve a single entry by namespace + key.
    async fn get(&self, namespace: Namespace, key: &str) -> Result<Option<Entry>, MemoryError>;

    /// Semantic search: embed the query, return the top `limit` entries sorted
    /// by descending cosine similarity.
    async fn search(
        &self,
        namespace: Namespace,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ScoredEntry>, MemoryError>;

    /// Delete an entry. No-op if it does not exist.
    async fn delete(&self, namespace: Namespace, key: &str) -> Result<(), MemoryError>;

    /// List entries in a namespace, newest first.
    async fn list(
        &self,
        namespace: Namespace,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Entry>, MemoryError>;
}

/// Convenience: wrap a `dyn Memory` in an `Arc` for sharing across tasks.
pub type SharedMemory = Arc<dyn Memory>;
