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
        _context: &OracleContext,
    ) -> anyhow::Result<OracleAdjudication> {
        // Step 1 skeleton: no backends registered yet → approve everything,
        // confidence 0.0 (no claims), zero attestations. The gate wiring in
        // `run_fanout` records this as advisory. When fleet backends land
        // (steps 3-5), this dispatches to the strongest registered backend.
        Ok(OracleAdjudication {
            phase,
            verdict: Verdict::Approve,
            confidence: 0.0,
            attestations: Vec::new(),
        })
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
