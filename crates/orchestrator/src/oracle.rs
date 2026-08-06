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
    model: String,
    gate: GateMode,
    block_confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    Advisory,
    Block,
}

impl FusionOracle {
    /// Build from config. Step 1: no fleet backends are contacted until
    /// `enabled = true` AND a backend is registered — a disabled Oracle is a
    /// no-op pass-through (today's behavior unchanged).
    pub fn new(model: impl Into<String>, gate: GateMode, block_confidence: f32) -> Self {
        Self {
            backends: Vec::new(),
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
                OracleBackend::Native { model } => {
                    // Native is always "alive"; it IS the fallback.
                    let _ = model;
                    true
                }
                _ => false, // OpenClaw/AgentField not wired yet (steps 4-5)
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

fn backend_label(b: &OracleBackend) -> String {
    match b {
        OracleBackend::Hermes { api, .. } => format!("hermes@{}", api),
        OracleBackend::Native { model } => format!("native@{}", model),
        OracleBackend::OpenClaw { endpoint } => format!("openclaw@{}", endpoint),
        OracleBackend::AgentField { control } => format!("agentfield@{}", control),
    }
}

impl FusionOracle {
    /// Step 3: the verdict comes from the Native adjudicator model. This is a
    /// stub that approves with 0.5 confidence — wiring an actual model call
    /// (the router-resolved `[oracle].model`) is the next increment.
    async fn native_adjudicate(
        &self,
        _phase: Phase,
        _context: &OracleContext,
    ) -> (Verdict, f32) {
        (Verdict::Approve, 0.5)
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
        config.oracle.model.clone(),
        gate,
        config.oracle.block_confidence,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // The Native backend never needs liveness — it IS the fallback. A
        // FusionOracle with only Native must dispatch (approve, 0.5, one
        // attestation naming the backend), not return the empty skeleton.
        let ctx = OracleContext::default();
        let mut o = FusionOracle::new("vaked/qwen3-coder:30b", GateMode::Advisory, 0.8);
        o.register_backend(OracleBackend::Native { model: "vaked/qwen3-coder:30b".into() });
        let adj = futures::executor::block_on(o.adjudicate(Phase::CoderDiff, &ctx)).unwrap();
        assert_eq!(adj.verdict, Verdict::Approve);
        assert_eq!(adj.confidence, 0.5);
        assert_eq!(adj.attestations.len(), 1);
        assert!(adj.attestations[0].proc.starts_with("native@"));
    }

    #[test]
    fn block_gate_only_fires_on_high_confidence_reject() {
        let o = FusionOracle::new("m", GateMode::Block, 0.8);
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
        let o = FusionOracle::new("m", GateMode::Advisory, 0.8);
        let adj = OracleAdjudication {
            phase: Phase::Integration,
            verdict: Verdict::Reject { reason: "z".into() },
            confidence: 1.0,
            attestations: vec![],
        };
        assert!(!o.would_block(&adj));
    }
}
