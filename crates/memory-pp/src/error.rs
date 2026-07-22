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
    /// Slice-2 sidecar protocol/spawn/timeout failure — carries a reason for the
    /// fallback log. (Distinct from `MeshUnavailable`, which is the no-sidecar stub.)
    #[error("mesh: {0}")]
    Mesh(String),
    #[error("marqant: {0}")]
    Marqant(String),
}
