//! Stage 2 — the 1-bit LLM mesh re-rank seam. In Slice 1 this is stubbed
//! in-process; Slice 2 drops in a stdio-JSON-RPC client over the existing
//! `crates/mcp` plumbing (method `rerank`, params {query, spans:[{id,text}],
//! deadline_ms, top_k?}, result {ranked_span_ids: <subset of input ids>, ...}).
//! The sidecar returns IDS ONLY — the Rust side rehydrates raw via RawStore::get,
//! preserving "never returns a rewritten payload".

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::PpError;
use crate::raw_store::{RawSpan, RawStore};

/// Cap on bytes read from the sidecar's stdout — a trusted in-repo process, but
/// bounded anyway (mirrors the shell/MCP capped-reader hardening).
const MAX_SIDECAR_STDOUT: u64 = 1 << 18; // 256 KiB

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

// ---- Slice 2: the real stdio-JSON-RPC sidecar client ------------------------

/// Build the single-line JSON-RPC `rerank` request the sidecar reads from stdin.
/// `candidates` are `(id, preview_text)`; the sidecar returns ids only.
fn build_rerank_request(
    query: &str,
    candidates: &[(String, String)],
    deadline_ms: u64,
    top_k: usize,
) -> String {
    let spans: Vec<Value> =
        candidates.iter().map(|(id, text)| json!({ "id": id, "text": text })).collect();
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "rerank",
        "params": { "query": query, "spans": spans, "deadline_ms": deadline_ms, "top_k": top_k },
    })
    .to_string() // compact → single line
}

/// Extract `result.ranked_span_ids` from the sidecar's stdout. Tolerates leading
/// blank lines; a JSON-RPC `error` object or a missing result is an `Err`
/// (→ fallback). Scans line-by-line so a stray trailing newline is harmless.
fn parse_rerank_response(stdout: &str) -> Result<Vec<String>, PpError> {
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue; // tolerate stray non-JSON lines
        };
        if let Some(err) = v.get("error") {
            return Err(PpError::Mesh(format!("sidecar error: {err}")));
        }
        if let Some(arr) =
            v.get("result").and_then(|r| r.get("ranked_span_ids")).and_then(Value::as_array)
        {
            return Ok(arr.iter().filter_map(|x| x.as_str().map(str::to_string)).collect());
        }
    }
    Err(PpError::Mesh("no ranked_span_ids in sidecar output".into()))
}

/// Reorder `spans` to follow `ids` (the sidecar's ranking), dropping ids the
/// sidecar invented (not in the candidate set) and candidates it didn't return.
/// Preserves the invariant "never introduces a span the caller didn't supply".
fn reorder_by_ids(spans: &[RawSpan], ids: &[String]) -> Vec<RawSpan> {
    let by_id: std::collections::HashMap<&str, &RawSpan> =
        spans.iter().map(|s| (s.id.as_str(), s)).collect();
    ids.iter().filter_map(|id| by_id.get(id.as_str()).map(|s| (*s).clone())).collect()
}

/// The production Slice-2 mesh: spawns `sidecar_cmd` as a stdio-JSON-RPC process
/// per request (stateless), sends the candidate previews, and applies the
/// returned ranking. Any spawn/protocol/timeout failure is an `Err` → the
/// processor falls back to top-K, so an absent or broken sidecar never regresses.
pub struct SidecarMesh {
    raw: RawStore,
    program: String,
    args: Vec<String>,
    preview_bytes: usize,
    top_k: usize,
}

impl SidecarMesh {
    /// `cmd` is whitespace-split into program + args (e.g. `"python serve.py"`).
    /// `raw` is a cheap clone-handle used to fetch candidate preview text.
    pub fn new(raw: RawStore, cmd: &str, preview_bytes: usize, top_k: usize) -> Self {
        let mut parts = cmd.split_whitespace().map(str::to_string);
        let program = parts.next().unwrap_or_default();
        let args: Vec<String> = parts.collect();
        SidecarMesh { raw, program, args, preview_bytes, top_k }
    }
}

/// Truncate to `max` bytes on a char boundary (never splits a UTF-8 codepoint).
fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[async_trait]
impl MeshSearch for SidecarMesh {
    async fn rerank(
        &self,
        query: &str,
        spans: &[RawSpan],
        deadline: Duration,
    ) -> Result<Vec<RawSpan>, PpError> {
        if self.program.is_empty() {
            return Err(PpError::Mesh("empty sidecar_cmd".into()));
        }
        // Fetch bounded preview text for each candidate (ids → text) so the sidecar
        // has something to rank; full raw is rehydrated by the processor afterward.
        let mut candidates: Vec<(String, String)> = Vec::with_capacity(spans.len());
        for s in spans {
            if let Some(rc) = self.raw.get(&s.id).await? {
                candidates.push((s.id.clone(), cap(&rc.bytes, self.preview_bytes)));
            }
        }
        if candidates.is_empty() {
            return Err(PpError::Mesh("no candidate text to rank".into()));
        }
        let req = build_rerank_request(query, &candidates, deadline.as_millis() as u64, self.top_k);
        let stdout = run_sidecar(&self.program, &self.args, &req).await?;
        let ids = parse_rerank_response(&stdout)?;
        Ok(reorder_by_ids(spans, &ids))
    }
}

// ---- Slice 2c: the native in-process mesh (no subprocess) -------------------

/// Feature-vector width: `[query hist(256) | text hist(256) | q⊙t interaction(256)]`.
/// The interaction block (element-wise product of the two histograms) is what lets a
/// *linear* `.ugm` reranker score a query×text match — a plain concat can't. A `.ugm`
/// reranker must consume exactly this many inputs.
pub const FEATURE_DIM: usize = 768;

/// Normalized byte histogram of `s` (each bin = count(byte)/len, 0 for empty).
fn byte_histogram(s: &str) -> Vec<f32> {
    let mut h = vec![0f32; 256];
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return h;
    }
    for &b in bytes {
        h[b as usize] += 1.0;
    }
    let inv = 1.0 / bytes.len() as f32;
    for v in &mut h {
        *v *= inv;
    }
    h
}

/// `[query_hist | text_hist | query_hist⊙text_hist]` — the `FEATURE_DIM`-wide input
/// to a `.ugm` reranker. The trailing interaction block carries the per-byte
/// co-occurrence a linear model needs to rank query×text relevance.
fn featurize(query: &str, text: &str) -> Vec<f32> {
    let q = byte_histogram(query);
    let t = byte_histogram(text);
    let mut f = Vec::with_capacity(FEATURE_DIM);
    f.extend_from_slice(&q);
    f.extend_from_slice(&t);
    f.extend(q.iter().zip(&t).map(|(a, b)| a * b));
    debug_assert_eq!(f.len(), FEATURE_DIM);
    f
}

/// Distinct lowercased-alphanumeric query terms.
fn query_terms(query: &str) -> std::collections::HashSet<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Deterministic lexical relevance: count of DISTINCT query terms (lowercased
/// alphanumeric runs) that appear in `text` (lowercased). The native fallback when
/// no `.ugm` model is configured — mirrors the sidecar's reference scorer, in Rust.
pub(crate) fn lexical_score(query: &str, text: &str) -> f32 {
    let text_lc = text.to_lowercase();
    query_terms(query).iter().filter(|t| text_lc.contains(t.as_str())).count() as f32
}

/// The native in-process mesh: reranks candidate spans without any subprocess.
/// Scores each candidate with a configured `.ugm` reranker (via `entheai-ultragraph`)
/// when present, else the lexical fallback. This is the default backend — it drops
/// the Python sidecar while keeping `SidecarMesh` available via config.
pub struct NativeMesh {
    raw: RawStore,
    model: Option<entheai_ultragraph::UgmFile>,
    preview_bytes: usize,
    top_k: usize,
}

impl NativeMesh {
    pub fn new(
        raw: RawStore,
        model: Option<entheai_ultragraph::UgmFile>,
        preview_bytes: usize,
        top_k: usize,
    ) -> Self {
        NativeMesh { raw, model, preview_bytes, top_k }
    }

    /// Load a `.ugm` reranker, accepting it only if every source tree (one with no
    /// incoming ultra-edge) takes exactly `FEATURE_DIM` inputs — otherwise the model
    /// can't consume our feature vector, so we reject it (→ lexical fallback) rather
    /// than risk a shape mismatch at run time.
    pub fn load_model(path: &std::path::Path) -> Option<entheai_ultragraph::UgmFile> {
        let m = entheai_ultragraph::UgmFile::load(path).ok()?;
        let has_src: std::collections::HashSet<u32> =
            m.ultra_edges.iter().map(|e| e.dst_idx).collect();
        let sources_ok = m
            .trees
            .iter()
            .enumerate()
            .filter(|(i, _)| !has_src.contains(&(*i as u32)))
            .all(|(_, t)| t.in_dim as usize == FEATURE_DIM);
        if sources_ok {
            Some(m)
        } else {
            None
        }
    }

    fn score(&self, query: &str, text: &str) -> f32 {
        match &self.model {
            Some(m) => {
                let feat = featurize(query, text);
                m.run(&[feat]).first().and_then(|row| row.first().copied()).unwrap_or(0.0)
            }
            None => lexical_score(query, text),
        }
    }
}

#[async_trait]
impl MeshSearch for NativeMesh {
    async fn rerank(
        &self,
        query: &str,
        spans: &[RawSpan],
        _deadline: Duration,
    ) -> Result<Vec<RawSpan>, PpError> {
        let mut scored: Vec<(usize, f32, RawSpan)> = Vec::with_capacity(spans.len());
        for (i, s) in spans.iter().enumerate() {
            if let Some(rc) = self.raw.get(&s.id).await? {
                let text = cap(&rc.bytes, self.preview_bytes);
                scored.push((i, self.score(query, &text), s.clone()));
            }
        }
        if scored.is_empty() {
            return Err(PpError::Mesh("no candidate text to rank".into()));
        }
        // Highest score first; stable on ties (original recall order via the index).
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0))
        });
        scored.truncate(self.top_k);
        Ok(scored.into_iter().map(|(_, _, s)| s).collect())
    }
}

/// Spawn the sidecar, write the request to its stdin, close stdin (EOF → the
/// stateless sidecar responds and exits), and read its stdout (capped). Stderr is
/// discarded. `kill_on_drop` guarantees a timed-out child is reaped when the
/// caller's outer deadline drops this future.
async fn run_sidecar(program: &str, args: &[String], req: &str) -> Result<String, PpError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::process::Command;

    let mut child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| PpError::Mesh(format!("spawn {program}: {e}")))?;

    {
        let mut stdin = child.stdin.take().ok_or_else(|| PpError::Mesh("no stdin".into()))?;
        stdin.write_all(req.as_bytes()).await.map_err(|e| PpError::Mesh(format!("write: {e}")))?;
        stdin.write_all(b"\n").await.map_err(|e| PpError::Mesh(format!("write: {e}")))?;
        // stdin dropped here → EOF for the sidecar.
    }

    let stdout = child.stdout.take().ok_or_else(|| PpError::Mesh("no stdout".into()))?;
    let mut buf = Vec::new();
    stdout
        .take(MAX_SIDECAR_STDOUT)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| PpError::Mesh(format!("read: {e}")))?;
    let _ = child.wait().await;
    Ok(String::from_utf8_lossy(&buf).into_owned())
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

    // ---- Slice 2: pure protocol helpers ----

    fn span(id: &str) -> RawSpan {
        use crate::raw_store::RawKind;
        RawSpan { id: id.into(), kind: RawKind::Transcript, score: 0.0, created_at: 0 }
    }

    #[test]
    fn request_carries_query_ids_and_budget() {
        let req = build_rerank_request(
            "the auth thing",
            &[("id1".into(), "auth login".into()), ("id2".into(), "disk usage".into())],
            1500,
            8,
        );
        let v: Value = serde_json::from_str(&req).expect("valid JSON line");
        assert_eq!(v["method"], "rerank");
        assert_eq!(v["params"]["query"], "the auth thing");
        assert_eq!(v["params"]["deadline_ms"], 1500);
        assert_eq!(v["params"]["top_k"], 8);
        assert_eq!(v["params"]["spans"][0]["id"], "id1");
        assert_eq!(v["params"]["spans"][0]["text"], "auth login");
        assert_eq!(v["params"]["spans"][1]["id"], "id2");
        assert!(!req.contains('\n'), "single-line request");
    }

    #[test]
    fn parses_ranked_ids_and_rejects_errors() {
        let ids = parse_rerank_response(
            "\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ranked_span_ids\":[\"b\",\"a\"]}}\n",
        )
        .unwrap();
        assert_eq!(ids, vec!["b".to_string(), "a".to_string()]);
        // A JSON-RPC error object → Err (fallback), not a silent empty ranking.
        assert!(parse_rerank_response("{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-1,\"message\":\"boom\"}}").is_err());
        assert!(parse_rerank_response("not json at all").is_err());
        assert!(parse_rerank_response("").is_err());
    }

    #[test]
    fn reorder_follows_ids_and_drops_unknowns() {
        let spans = vec![span("a"), span("b"), span("c")];
        // sidecar ranks c,a and invents "z" (must be ignored); "b" not returned → dropped.
        let out = reorder_by_ids(&spans, &["c".into(), "z".into(), "a".into()]);
        assert_eq!(out.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["c", "a"]);
    }

    // ---- Slice 2: subprocess glue ----

    #[tokio::test]
    async fn sidecar_missing_binary_errors_to_fallback() {
        use crate::raw_store::{RawKind, RawStore};
        let raw = RawStore::open_memory().unwrap();
        let id = raw.ingest(RawKind::Transcript, "auth login flow", None).await.unwrap();
        let mesh = SidecarMesh::new(raw, "definitely-not-a-real-binary-xyz", 4096, 8);
        let r = mesh.rerank("auth", &[span(&id)], Duration::from_millis(200)).await;
        assert!(matches!(r, Err(PpError::Mesh(_))), "absent sidecar → Err → fallback");
    }

    // Real subprocess round-trip against the in-repo Python sidecar. Proves the
    // client's fetch→write→read→parse→reorder path end to end. Ignored by default
    // (needs python3); run with `cargo test -- --ignored` / in the CI gate.
    #[tokio::test]
    #[ignore = "integration: needs python3 + sidecars/ultragraph/serve.py"]
    async fn sidecar_roundtrip_via_serve_py() {
        use crate::raw_store::{RawKind, RawStore};
        // Resolve serve.py relative to the workspace root (two levels up from the crate).
        let serve = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sidecars/ultragraph/serve.py");
        let raw = RawStore::open_memory().unwrap();
        let a = raw.ingest(RawKind::Transcript, "the auth login flow and tokens", None).await.unwrap();
        let b = raw.ingest(RawKind::Transcript, "unrelated disk usage report", None).await.unwrap();
        let spans = vec![span(&b), span(&a)]; // deliberately auth-second
        let mesh = SidecarMesh::new(raw, &format!("python3 {serve}"), 8192, 8);
        let out = mesh.rerank("auth tokens", &spans, Duration::from_millis(4000)).await.unwrap();
        assert!(!out.is_empty(), "sidecar returned a ranking");
        // The reference scorer ranks the auth span first for an auth query.
        assert_eq!(out[0].id, a, "auth-relevant span ranked first");
    }

    // ---- Slice 2c: native in-process mesh ----

    #[test]
    fn byte_histogram_is_normalized() {
        let h = byte_histogram("aab");
        assert_eq!(h.len(), 256);
        assert!((h[b'a' as usize] - 2.0 / 3.0).abs() < 1e-6);
        assert!((h[b'b' as usize] - 1.0 / 3.0).abs() < 1e-6);
        let sum: f32 = h.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "histogram sums to 1");
        assert_eq!(byte_histogram("").iter().sum::<f32>(), 0.0, "empty → all zero");
    }

    #[test]
    fn featurize_has_query_text_and_interaction() {
        // "ab" vs "ac": q={a:.5,b:.5}, t={a:.5,c:.5}. The interaction (q⊙t) block
        // is what lets a *linear* reranker score a query×text match — only byte 'a'
        // is shared, so only its product bin is non-zero.
        let f = featurize("ab", "ac");
        assert_eq!(f.len(), FEATURE_DIM);
        assert!((f[b'a' as usize] - 0.5).abs() < 1e-6, "query half");
        assert!((f[256 + b'c' as usize] - 0.5).abs() < 1e-6, "text half");
        assert!((f[512 + b'a' as usize] - 0.25).abs() < 1e-6, "interaction: shared byte 'a'");
        assert!(f[512 + b'b' as usize].abs() < 1e-6, "interaction: 'b' not in text → 0");
    }

    #[test]
    fn lexical_score_counts_distinct_query_terms() {
        assert_eq!(lexical_score("auth token", "the auth login and token flow"), 2.0);
        assert_eq!(lexical_score("AUTH", "auth auth auth"), 1.0, "distinct, case-insensitive");
        assert_eq!(lexical_score("disk", "the auth login flow"), 0.0);
    }

    #[tokio::test]
    async fn native_lexical_ranks_relevant_span_first() {
        use crate::raw_store::{RawKind, RawStore};
        let raw = RawStore::open_memory().unwrap();
        let disk = raw.ingest(RawKind::ToolOutput, "unrelated disk usage report", None).await.unwrap();
        let auth = raw.ingest(RawKind::Transcript, "the auth login flow and tokens", None).await.unwrap();
        let mesh = NativeMesh::new(raw, None, 8192, 8);
        // deliberately present the auth span second
        let out = mesh
            .rerank("auth tokens", &[span(&disk), span(&auth)], Duration::from_millis(50))
            .await
            .unwrap();
        assert_eq!(out[0].id, auth, "native lexical ranks the auth span first");
    }

    #[tokio::test]
    async fn native_uses_ugm_model_to_score() {
        use crate::raw_store::{RawKind, RawStore};
        use entheai_ultragraph::ugm::{ACT_NONE, KIND_DENSE};
        use entheai_ultragraph::{UgmFile, UgmTree};
        // A FEATURE_DIM->1 dense reranker whose score = the text histogram bin for
        // byte 'z' (weight +1 at index 256 + 'z'). So more 'z's in the raw span →
        // higher score. Proves featurize→UGM run drives the ranking.
        let mut wq = vec![0i8; FEATURE_DIM];
        wq[256 + b'z' as usize] = 1;
        let tree = UgmTree {
            kind: KIND_DENSE,
            act: ACT_NONE,
            in_dim: FEATURE_DIM as u32,
            out_dim: 1,
            name: "rerank".into(),
            w_scale: 1.0,
            wq: Some(wq),
            bias: Some(vec![0.0]),
        };
        let model = UgmFile { trees: vec![tree], ultra_edges: vec![] };

        let raw = RawStore::open_memory().unwrap();
        let plain = raw.ingest(RawKind::Transcript, "aaaa bbbb", None).await.unwrap();
        let zzz = raw.ingest(RawKind::Transcript, "zzz zebra", None).await.unwrap();
        let mesh = NativeMesh::new(raw, Some(model), 8192, 8);
        let out = mesh
            .rerank("anything", &[span(&plain), span(&zzz)], Duration::from_millis(50))
            .await
            .unwrap();
        assert_eq!(out[0].id, zzz, "the 'z'-heavy span scores higher under the model");
    }
}
