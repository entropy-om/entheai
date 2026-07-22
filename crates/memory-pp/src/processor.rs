//! The prompt-processing orchestrator: recall → rerank → rehydrate raw →
//! compress. Every non-happy branch returns `Ok(None)` (or propagates `Err`),
//! which the core call site treats as "fall back to today's top-K". Ingest is
//! best-effort (log + swallow) so it can never fail a task.
//!
//! Fail-safe (adversarial-review correction #2): the WHOLE pipeline — recall, the
//! rehydrate loop, mesh, marqant, and any `Arc<Mutex<Connection>>` contention — is
//! bounded by ONE overall `deadline`, so a *slow* path degrades to top-K, not only
//! an erroring one.
//!
//! Slice-1 success-path contract (documented now so Slice 2 isn't a silent
//! divergence): the brief is the compressor's output injected VERBATIM as the
//! system-message content. It does NOT reuse top-K's `[label score= key=]` block
//! format — the marqant `.mq` brief is itself the injectable body. In Slice 1
//! `StubMesh` short-circuits, so the success path never fires in production; the
//! ingest side is what is live and testable.

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
    /// Correction #4: cap unbounded tool/transcript payloads before they hit the
    /// raw store, so a single megabyte-scale tool output can't balloon `raw.db`
    /// within one run (mirrors the shell/MCP capped-reader precedent).
    max_ingest_bytes: usize,
}

/// Truncate `s` to at most `max` bytes on a char boundary (never panics mid-char).
fn cap_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

impl PromptProcessor {
    pub fn new(
        raw: RawStore,
        mesh: Box<dyn MeshSearch>,
        marqant: Box<dyn Marqant>,
        deadline: Duration,
        recall_k: usize,
        max_ingest_bytes: usize,
    ) -> Self {
        Self {
            raw,
            mesh,
            marqant,
            deadline,
            recall_k,
            max_ingest_bytes,
        }
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
    ///
    /// Correction #2: one overall deadline around the entire pipeline. On elapse →
    /// `Ok(None)` (fall back), so a slow recall/rehydrate/lock never hangs a prompt.
    pub async fn retrieve(&self, msg: &str) -> Result<Option<String>, PpError> {
        if msg.trim().is_empty() {
            return Ok(None);
        }
        match tokio::time::timeout(self.deadline, self.retrieve_inner(msg)).await {
            Ok(r) => r,
            Err(_) => {
                warn!("pp deadline exceeded → falling back to top-K");
                Ok(None)
            }
        }
    }

    async fn retrieve_inner(&self, msg: &str) -> Result<Option<String>, PpError> {
        // Stage 1 — cheap, wide lexical recall.
        let candidates = self.raw.recall(msg, self.recall_k).await?;
        if candidates.is_empty() {
            return Ok(None); // empty raw store / no lexical hit → fallback
        }
        // Stage 2 — mesh re-rank. Mesh error → Ok(None) (fallback), never Err: an
        // experimental-path failure must not become fatal even under strict mode.
        // (`deadline` is passed for the Slice-2 subprocess bound; the outer timeout
        // above is the real guard against a slow mesh.)
        let ranked = match self.mesh.rerank(msg, &candidates, self.deadline).await {
            Ok(r) => r,
            Err(e) => {
                warn!("pp mesh error → falling back to top-K: {e}");
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
        // Stage 3 — deterministic compression. Error/empty brief → fallback (an
        // empty brief must never be injected as "success").
        match self.marqant.compress(&findings, self.deadline).await {
            Ok(brief) if !brief.trim().is_empty() => Ok(Some(brief)),
            Ok(_) => Ok(None),
            Err(e) => {
                warn!("pp marqant error → falling back to top-K: {e}");
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
        let body = cap_bytes(&ev.result, self.max_ingest_bytes);
        if let Err(e) = self.raw.ingest(RawKind::ToolOutput, body, Some(meta)).await {
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
        let body = cap_bytes(&buf, self.max_ingest_bytes);
        if let Err(e) = self.raw.ingest(RawKind::Transcript, body, Some(meta)).await {
            warn!("pp ingest_transcript failed (continuing): {e}");
        }
    }
}

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
        PromptProcessor::new(
            raw,
            mesh,
            Box::new(StubMarqant),
            Duration::from_millis(50),
            16,
            1 << 20,
        )
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
        pp.raw()
            .ingest(RawKind::Transcript, "the auth thing", None)
            .await
            .unwrap();
        assert_eq!(
            pp.retrieve("auth").await.unwrap(),
            None,
            "mesh err → fallback signal"
        );
    }

    #[tokio::test]
    async fn slow_mesh_times_out_to_fallback() {
        let pp = pp_with(Box::new(SlowStubMesh {
            sleep: Duration::from_millis(300),
        }));
        pp.raw()
            .ingest(RawKind::Transcript, "auth login flow", None)
            .await
            .unwrap();
        assert_eq!(
            pp.retrieve("auth").await.unwrap(),
            None,
            "deadline → fallback signal"
        );
    }

    #[tokio::test]
    async fn happy_path_produces_brief_from_raw() {
        let pp = pp_with(Box::new(IdentityMesh));
        pp.raw()
            .ingest(RawKind::Transcript, "auth login flow details", None)
            .await
            .unwrap();
        let brief = pp.retrieve("auth").await.unwrap().expect("brief");
        assert!(
            brief.contains("auth login flow details"),
            "brief carries the raw finding"
        );
    }

    #[tokio::test]
    async fn native_mesh_runs_pipeline_fully_in_process() {
        // The default Slice-2c backend: recall → NATIVE rerank (no subprocess) →
        // rehydrate raw → compress. Proves PP produces a real brief with zero
        // external tools (native mesh + identity StubMarqant).
        use crate::mesh::NativeMesh;
        let raw = RawStore::open_memory().unwrap();
        let mesh = NativeMesh::new(raw.clone(), None, 4096, 8);
        let pp = PromptProcessor::new(
            raw,
            Box::new(mesh),
            Box::new(StubMarqant),
            Duration::from_millis(200),
            16,
            1 << 20,
        );
        pp.raw()
            .ingest(RawKind::ToolOutput, "unrelated disk usage report", None)
            .await
            .unwrap();
        pp.raw()
            .ingest(RawKind::Transcript, "the auth login and token flow", None)
            .await
            .unwrap();
        let brief = pp
            .retrieve("auth token")
            .await
            .unwrap()
            .expect("in-process brief");
        assert!(
            brief.contains("auth login and token flow"),
            "native mesh surfaced the auth finding"
        );
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
        assert_eq!(
            pp.raw().count().await.unwrap(),
            2,
            "one tool row + one transcript row"
        );
    }

    #[tokio::test]
    #[ignore = "integration: exercised in the full suite / CI gate"]
    async fn slice1_end_to_end_falls_back_and_ingest_is_idempotent() {
        let pp = pp_with(Box::new(StubMesh)); // production stub

        // Simulate a run's ingest.
        let sc = scope();
        let msgs = vec![entheai_providers::ChatMessage::user("fix the auth bug")];
        pp.ingest_transcript(&sc, &msgs, "fixed it").await;
        let ev = entheai_memory::ToolEvidence {
            call_id: "c".into(),
            name: "run_shell".into(),
            args: "grep auth".into(),
            result: "auth.rs:42".into(),
            allowed: true,
        };
        pp.ingest_tool(&sc, &ev).await;
        assert_eq!(pp.raw().count().await.unwrap(), 2);

        // Retrieval with the production stub → fallback signal (core uses top-K).
        assert_eq!(pp.retrieve("auth").await.unwrap(), None);

        // Re-running the same session ingests nothing new (content-addressed).
        pp.ingest_transcript(&sc, &msgs, "fixed it").await;
        pp.ingest_tool(&sc, &ev).await;
        assert_eq!(
            pp.raw().count().await.unwrap(),
            2,
            "idempotent across re-runs"
        );
    }
}
