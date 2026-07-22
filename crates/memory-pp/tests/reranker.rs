//! Acceptance test for the shipped `.ugm` ternary reranker: NativeMesh must LOAD
//! `models/reranker.ugm` (proving the ultra-graph train → export → Rust-load loop)
//! and rank a held-out relevant span above an irrelevant one for a topical query.
//!
//! `#[ignore]`d until the model is committed; the training pipeline that produces it
//! is `tools/train_reranker.py`. Run with `cargo test -p entheai-memory-pp -- --ignored`.

use std::path::PathBuf;
use std::time::Duration;

use entheai_memory_pp::{MeshSearch, NativeMesh, RawKind, RawSpan, RawStore};

fn model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/reranker.ugm")
}

#[tokio::test]
async fn trained_ugm_reranker_ranks_relevant_first() {
    // The model must load and be accepted (FEATURE_DIM-shaped) — a None here means
    // the reranker is missing or the wrong shape, which is a real failure, not a skip.
    let model = NativeMesh::load_model(&model_path())
        .expect("models/reranker.ugm loads and is FEATURE_DIM-shaped");

    let raw = RawStore::open_memory().unwrap();
    let irrelevant = raw
        .ingest(
            RawKind::ToolOutput,
            "disk usage report: 42% full on /dev/sda1",
            None,
        )
        .await
        .unwrap();
    let relevant = raw
        .ingest(
            RawKind::Transcript,
            "the authentication login token refresh flow",
            None,
        )
        .await
        .unwrap();

    let mesh = NativeMesh::new(raw, Some(model), 8192, 8);
    let span = |id: &str| RawSpan {
        id: id.into(),
        kind: RawKind::Transcript,
        score: 0.0,
        created_at: 0,
    };
    // Present the relevant span SECOND so a passing result reflects scoring, not order.
    let out = mesh
        .rerank(
            "auth login token",
            &[span(&irrelevant), span(&relevant)],
            Duration::from_millis(200),
        )
        .await
        .unwrap();
    assert_eq!(
        out[0].id, relevant,
        "trained reranker ranks the auth span first"
    );
}
