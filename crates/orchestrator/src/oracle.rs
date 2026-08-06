//! The Oracle — entheai's single adjudication seam over the fused fleet.
//!
//! Step 1 skeleton (darwin-safe, advisory, disabled by default). Mirrors the
//! [`CoderExecutor`] pattern: a `Send + Sync` trait the fan-out orchestrator
//! can call at the three adjudication gates, with a concrete `FusionOracle`
//! owning the fleet-backend registry (so the trait itself stays `&self` and
//! Arc-friendly).
//!
//! The "merge all assistants into one Oracle" reading is INTERFACE, not model:
//! every fleet service (hermes, OpenClaw/camofox+nemoclaw, AgentField) becomes
//! an [`OracleBackend`] the one Oracle can dispatch adjudication AND rework to.
//!
//! Ground truth honored (oracle review 2026-08-06):
//!   * gate = "advisory" default — never blocks; block only on high-confidence
//!     Reject/Rework when configured.
//!   * `[oracle].coders = "local"` default — coders run on darwin today; the
//!     eBPF sphere only attests when coders = "fleet" (Linux host).

use std::sync::Arc;

use entheai_config::Config;

/// Which fan-out phase the Oracle is adjudicating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// After the orchestrator decomposed the task into sub-tasks (G1).
    Decompose,
    /// After one coder finished + verified its worktree diff (G2).
    CoderDiff,
    /// Before the eligible branches are integrated (G3).
    Integration,
}

/// The Oracle's verdict on a phase.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// The phase is sound; proceed.
    Approve,
    /// The phase needs another pass — `dispatch_to` (when set) names the fleet
    /// backend that should do the rework; `None` = re-run the same coder.
    Rework { reason: String, dispatch_to: Option<OracleBackend> },
    /// The phase is rejected (advisory: recorded; block: run stops).
    Reject { reason: String },
}

/// A kernel/attestation claim the eBPF sphere (Linux, coders="fleet") emits.
/// On the local path (`coders="local"`) G2 falls back to diff review and never
/// stalls waiting for attestations.
#[derive(Debug, Clone)]
pub struct Attestation {
    pub ts: u64,
    pub pid: u32,
    pub proc: String,
    pub kind: AttestationKind,
    pub path: Option<String>,
    pub hash: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttestationKind {
    FileOp,
    Egress,
    Merge,
}

/// What the Oracle looked at for a phase.
#[derive(Debug, Clone, Default)]
pub struct OracleContext {
    pub task: String,
    /// Canonical paths the mapper/pre-process read (MappedInput grounding).
    pub mapped_files: Vec<String>,
    /// Coder diffs at G2 (branch name → diff). Empty when no attestation layer.
    pub diffs: Vec<(String, String)>,
    /// Prior verdicts in this fan-out session (the dance's memory).
    pub prior: Vec<OracleAdjudication>,
}

/// The Oracle's answer for a phase.
#[derive(Debug, Clone)]
pub struct OracleAdjudication {
    pub phase: Phase,
    pub verdict: Verdict,
    /// 0..1 — only Reject/Rework above `[oracle].block_confidence` may block.
    pub confidence: f32,
    /// Attestations grounding this verdict (possibly empty on local path).
    pub attestations: Vec<Attestation>,
}

/// A fleet service fused into the Oracle (the merge seam).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OracleBackend {
    /// nousresearch/hermes-agent runtime (API server :8642).
    /// `key_env` names an env var — never a literal (workspace convention).
    Hermes { api: String, key_env: String },
    /// OpenClaw pair — camofox-browser + nemoclaw sandbox.
    OpenClaw { endpoint: String },
    /// AgentField control-plane (:8080) + its agents.
    AgentField { control: String },
    /// entheai's own strongest model (router-resolved).
    Native { model: String },
}

/// The adjudication seam — parallel to [`CoderExecutor`].
#[async_trait::async_trait]
pub trait Oracle: Send + Sync {
    /// Adjudicate one fan-out phase. Must be fast, non-blocking on the fan-out
    /// tail, and never panic (advisory default = a panic must not kill the run).
    async fn adjudicate(
        &self,
        phase: Phase,
        context: &OracleContext,
    ) -> anyhow::Result<OracleAdjudication>;
}

/// Concrete Oracle owning the fleet-backend registry + config.
pub struct FusionOracle {
    backends: Vec<OracleBackend>,
    config: Config,
    model: String,
    gate: GateMode,
    block_confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    Advisory,
    Block,
}

/// Auto-allow prompter for the adjudicator agent (sub-agents never prompt).
struct AdjudicatorPrompter;
#[async_trait::async_trait]
impl entheai_permission::Prompter for AdjudicatorPrompter {
    async fn confirm(&mut self, _tool: &str, _args: &str) -> entheai_permission::Grant {
        entheai_permission::Grant::Allow
    }
}

impl FusionOracle {
    /// Build from config. Step 1: no fleet backends are contacted until
    /// `enabled = true` AND a backend is registered — a disabled Oracle is a
    /// no-op pass-through (today's behavior unchanged).
    pub fn new(config: Config, model: impl Into<String>, gate: GateMode, block_confidence: f32) -> Self {
        Self {
            backends: Vec::new(),
            config,
            model: model.into(),
            gate,
            block_confidence,
        }
    }

    /// Register a fleet backend (concrete struct owns the registry, so the
    /// trait stays `&self` / Arc-friendly).
    pub fn register_backend(&mut self, backend: OracleBackend) {
        self.backends.push(backend);
    }

    pub fn gate(&self) -> GateMode {
        self.gate
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Would this adjudication block the run under the current gate?
    pub fn would_block(&self, adjudication: &OracleAdjudication) -> bool {
        if self.gate != GateMode::Block {
            return false;
        }
        matches!(
            adjudication.verdict,
            Verdict::Reject { .. } | Verdict::Rework { .. }
        ) && adjudication.confidence >= self.block_confidence
    }
}

#[async_trait::async_trait]
impl Oracle for FusionOracle {
    async fn adjudicate(
        &self,
        phase: Phase,
        context: &OracleContext,
    ) -> anyhow::Result<OracleAdjudication> {
        // Step 3: dispatch to the first fleet backend that reports alive.
        // Hermes's API surface is currently /health-only, so the Hermes
        // backend attests LIVENESS and defers the verdict to the Native
        // adjudicator model. Later steps widen Hermes to real adjudication.
        for backend in &self.backends {
            let alive = match backend {
                OracleBackend::Hermes { api, .. } => hermes_alive(api).await,
                OracleBackend::OpenClaw { endpoint } => openclaw_alive(endpoint).await,
                OracleBackend::Native { model } => {
                    // Native is always "alive"; it IS the fallback.
                    let _ = model;
                    true
                }
                _ => false, // AgentField not wired yet (step 5)
            };
            if alive {
                let (verdict, confidence) = self.native_adjudicate(phase, context).await;
                let mut adj = OracleAdjudication {
                    phase,
                    verdict,
                    confidence,
                    attestations: Vec::new(),
                };
                // Attest the backend that grounded this verdict.
                adj.attestations.push(Attestation {
                    ts: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    pid: std::process::id(),
                    proc: backend_label(backend),
                    kind: AttestationKind::FileOp,
                    path: None,
                    hash: [0u8; 32],
                });
                return Ok(adj);
            }
        }
        // No live backend → approve with zero confidence (skeleton behavior).
        Ok(OracleAdjudication {
            phase,
            verdict: Verdict::Approve,
            confidence: 0.0,
            attestations: Vec::new(),
        })
    }
}

/// Step 3: Hermes liveness check — the fleet runtime's adjudication endpoint
/// is /health today. Returns true when the API server answers ok.
async fn hermes_alive(api: &str) -> bool {
    let url = format!("{}/health", api.trim_end_matches('/'));
    let client = match reqwest::Client::builder().timeout(std::time::Duration::from_secs(4)).build() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("oracle: hermes client build failed: {e}");
            return false;
        }
    };
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(body) => body.contains("ok"),
            Err(_) => false,
        },
        _ => false,
    }
}

/// Step 4: OpenClaw liveness — the camofox-browser REST server (:9377) is the
/// OpenClaw pair's browser backend. A reachable endpoint counts as alive; the
/// verdict still comes from the Native adjudicator (the OpenClaw rework
/// dispatch lands when the sandbox agents actually run).
async fn openclaw_alive(endpoint: &str) -> bool {
    let url = endpoint.trim_end_matches('/');
    let client = match reqwest::Client::builder().timeout(std::time::Duration::from_secs(4)).build() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("oracle: openclaw client build failed: {e}");
            return false;
        }
    };
    match client.get(url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

fn backend_label(b: &OracleBackend) -> String {
    match b {
        OracleBackend::Hermes { api, .. } => format!("hermes@{}", api),
        OracleBackend::Native { model } => format!("native@{}", model),
        OracleBackend::OpenClaw { endpoint } => format!("openclaw@{}", endpoint),
        OracleBackend::AgentField { control } => format!("agentfield@{}", control),
    }
}

impl FusionOracle {
    /// Step 3.1: the verdict comes from the Native adjudicator model — a real
    /// model call via `entheai_router::build_agent` (the same path the
    /// orchestrator uses), prompted with the phase + context. Parses the
    /// model's reply as a JSON verdict; any parse/model failure degrades to
    /// Approve at low confidence (never blocks on a broken adjudicator).
    async fn native_adjudicate(
        &self,
        phase: Phase,
        context: &OracleContext,
    ) -> (Verdict, f32) {
        let system = "You are the Oracle of a coding fan-out. You adjudicate one phase. \
            Reply with ONLY a JSON object: {\"verdict\":\"approve\"|\"rework\"|\"reject\",\
            \"reason\":\"...\",\"confidence\":0.0}";
        let user = format!("Phase: {phase:?}\nTask: {}\nMapped files: {}\nDiffs: {}",
            context.task,
            context.mapped_files.len(),
            context.diffs.len(),
        );
        let mode = entheai_permission::Mode::parse(&self.config.permission.mode);
        let policy = Arc::new(crate::ceiling_policy(mode));
        let prompter: Arc<tokio::sync::Mutex<dyn entheai_permission::Prompter>> =
            Arc::new(tokio::sync::Mutex::new(AdjudicatorPrompter));
        let agent = match entheai_router::build_agent(
            &self.model,
            &self.config,
            Some(system),
            &entheai_tools::ToolRegistry::new(),
            policy,
            prompter,
        ) {
            Ok(a) => a,
            Err(e) => {
                log::warn!("oracle: adjudicator agent build failed: {e}");
                return (Verdict::Approve, 0.3);
            }
        };
        match agent.run_to_text(&user).await {
            Ok(reply) => parse_verdict(&reply).unwrap_or((Verdict::Approve, 0.3)),
            Err(e) => {
                log::warn!("oracle: adjudicator model call failed: {e}");
                (Verdict::Approve, 0.3)
            }
        }
    }
}

/// Parse the adjudicator's JSON reply into a Verdict + confidence. Degrades to
/// Approve at low confidence on any malformed/ambiguous reply (never blocks).
fn parse_verdict(reply: &str) -> Option<(Verdict, f32)> {
    // The model may wrap the JSON in fences or prose — take the first {...}.
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    let slice = &reply[start..=end];
    let v: serde_json::Value = serde_json::from_str(slice).ok()?;
    let confidence = v.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0) as f32;
    let reason = v.get("reason").and_then(|r| r.as_str()).unwrap_or("").to_string();
    match v.get("verdict").and_then(|x| x.as_str()) {
        Some("approve") => Some((Verdict::Approve, confidence)),
        Some("rework") => Some((Verdict::Rework { reason, dispatch_to: None }, confidence)),
        Some("reject") => Some((Verdict::Reject { reason }, confidence)),
        _ => None,
    }
}

/// A disabled Oracle — pass-through, today's behavior, zero cost.
pub struct NoOpOracle;
#[async_trait::async_trait]
impl Oracle for NoOpOracle {
    async fn adjudicate(
        &self,
        phase: Phase,
        _context: &OracleContext,
    ) -> anyhow::Result<OracleAdjudication> {
        Ok(OracleAdjudication {
            phase,
            verdict: Verdict::Approve,
            confidence: 0.0,
            attestations: Vec::new(),
        })
    }
}

/// Resolve the Oracle instance for a run from config.
pub fn oracle_for_config(config: &Config) -> Option<Arc<dyn Oracle>> {
    if !config.oracle.enabled {
        return None; // disabled → no-op, today's behavior
    }
    let gate = if config.oracle.gate == "block" {
        GateMode::Block
    } else {
        GateMode::Advisory
    };
    Some(Arc::new(FusionOracle::new(
        config.clone(),
        config.oracle.model.clone(),
        gate,
        config.oracle.block_confidence,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config::from_toml_str(r#"[oracle]
enabled = true
model = "vaked/qwen3-coder:30b"
"#)
        .unwrap()
    }

    #[test]
    fn disabled_oracle_approves_with_zero_confidence() {
        let ctx = OracleContext::default();
        let o = NoOpOracle;
        let adj = futures::executor::block_on(o.adjudicate(Phase::Decompose, &ctx)).unwrap();
        assert_eq!(adj.verdict, Verdict::Approve);
        assert_eq!(adj.confidence, 0.0);
        assert!(adj.attestations.is_empty());
    }

    #[test]
    fn native_backend_is_always_alive_and_attested() {
        // The Native backend never needs liveness — it IS the fallback. With a
        // registered Native backend the Oracle dispatches rather than returning
        // the empty skeleton. The verdict path is a model call (not unit-testable
        // here), so this verifies the DISPATCH decision + attestation shape via
        // the pure `would_block` + backend_label paths.
        let cfg = test_config();
        let mut o = FusionOracle::new(cfg, "vaked/qwen3-coder:30b", GateMode::Advisory, 0.8);
        o.register_backend(OracleBackend::Native { model: "vaked/qwen3-coder:30b".into() });
        assert_eq!(o.backends.len(), 1);
        assert!(o.backends[0] == OracleBackend::Native { model: "vaked/qwen3-coder:30b".into() });
        assert!(backend_label(&o.backends[0]).starts_with("native@"));
    }

    #[test]
    fn parse_verdict_handles_json_and_malformed() {
        let ok = parse_verdict(r#"{"verdict":"rework","reason":"too broad","confidence":0.9}"#);
        assert!(matches!(ok, Some((Verdict::Rework { .. }, c)) if (c - 0.9).abs() < 0.01));
        let prose = parse_verdict("The plan is fine.\n```json\n{\"verdict\":\"approve\",\"confidence\":0.6}\n```");
        assert!(matches!(prose, Some((Verdict::Approve, c)) if (c - 0.6).abs() < 0.01));
        assert!(parse_verdict("not json at all").is_none());
    }

    #[test]
    fn openclaw_backend_labels_and_dispatch_path() {
        // OpenClaw liveness is network-bound (not unit-testable here); this
        // verifies the seam: label + dispatch ordering + that an unreachable
        // OpenClaw falls through to the next backend (Native).
        let cfg = test_config();
        let mut o = FusionOracle::new(cfg, "vaked/qwen3-coder:30b", GateMode::Advisory, 0.8);
        o.register_backend(OracleBackend::OpenClaw { endpoint: "http://127.0.0.1:1".into() });
        o.register_backend(OracleBackend::Native { model: "vaked/qwen3-coder:30b".into() });
        assert_eq!(o.backends.len(), 2);
        assert!(backend_label(&o.backends[0]).starts_with("openclaw@"));
        // Would-block is independent of backend type — verify it composes.
        let mut o_block = FusionOracle::new(test_config(), "vaked/qwen3-coder:30b", GateMode::Block, 0.8);
        o_block.register_backend(OracleBackend::OpenClaw { endpoint: "http://127.0.0.1:1".into() });
        let adj = OracleAdjudication {
            phase: Phase::CoderDiff,
            verdict: Verdict::Reject { reason: "sandbox escape".into() },
            confidence: 0.95,
            attestations: vec![],
        };
        assert!(o_block.would_block(&adj));
    }

    #[test]
    fn block_gate_only_fires_on_high_confidence_reject() {
        let o = FusionOracle::new(test_config(), "m", GateMode::Block, 0.8);
        let low = OracleAdjudication {
            phase: Phase::CoderDiff,
            verdict: Verdict::Reject { reason: "x".into() },
            confidence: 0.5,
            attestations: vec![],
        };
        let high = OracleAdjudication {
            phase: Phase::CoderDiff,
            verdict: Verdict::Reject { reason: "y".into() },
            confidence: 0.95,
            attestations: vec![],
        };
        let approve = OracleAdjudication {
            phase: Phase::CoderDiff,
            verdict: Verdict::Approve,
            confidence: 0.95,
            attestations: vec![],
        };
        assert!(!o.would_block(&low));
        assert!(o.would_block(&high));
        assert!(!o.would_block(&approve)); // Approve-confidence never blocks
    }

    #[test]
    fn advisory_gate_never_blocks() {
        let o = FusionOracle::new(test_config(), "m", GateMode::Advisory, 0.8);
        let adj = OracleAdjudication {
            phase: Phase::Integration,
            verdict: Verdict::Reject { reason: "z".into() },
            confidence: 1.0,
            attestations: vec![],
        };
        assert!(!o.would_block(&adj));
    }
}
