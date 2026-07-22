//! Stage 3 — deterministic compression seam (marqant `mq`). Slice-1 stub is an
//! identity passthrough (never reached on the live path — StubMesh short-circuits
//! before it). Slice 2 swaps in the `mq compress <in.md> -o <out.mq> --semantic`
//! subprocess (file-arg I/O, `--semantic` yes / `--binary` no, capped reader +
//! timeout mirroring crates/tools/src/shell.rs; deterministic, golden-testable).

use std::time::Duration;

use async_trait::async_trait;

use crate::error::PpError;

#[async_trait]
pub trait Marqant: Send + Sync {
    /// Deterministically distil raw findings into the injectable brief.
    /// `deadline` bounds the Slice-2 `mq` subprocess (the impl owns `kill_on_drop`)
    /// — symmetric with `MeshSearch::rerank` so Slice 2 drops in without a trait
    /// change or orphaned `mq` processes on timeout.
    async fn compress(&self, findings: &str, deadline: Duration) -> Result<String, PpError>;
}

pub struct StubMarqant;

#[async_trait]
impl Marqant for StubMarqant {
    async fn compress(&self, findings: &str, _deadline: Duration) -> Result<String, PpError> {
        Ok(findings.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn stub_marqant_is_identity() {
        assert_eq!(
            StubMarqant.compress("brief body", Duration::from_millis(10)).await.unwrap(),
            "brief body"
        );
    }
}
