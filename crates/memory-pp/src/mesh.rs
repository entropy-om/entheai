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
