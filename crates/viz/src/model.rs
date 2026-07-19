//! The swarm state machine: a deterministic model of a fan-out run, folded from
//! semantic mutator calls (the TUI maps `FanoutEvent`s onto these). No clock, no
//! I/O, no orchestrator dependency — trivially unit-testable.

/// Lifecycle status of one sub-agent node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Pending,
    Running,
    Done,
    Failed,
}

/// One sub-agent in the swarm.
#[derive(Debug, Clone)]
pub struct SwarmNode {
    pub index: usize,
    pub role: String,
    pub task: String,
    pub status: NodeStatus,
    pub committed: bool,
}

/// Overall phase of the fan-out run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Fanning,
    Integrating,
    Done,
}

/// The full swarm model. `Default` = an idle, empty swarm.
#[derive(Debug, Clone, Default)]
pub struct SwarmModel {
    pub nodes: Vec<SwarmNode>,
    pub phase: Phase,
    pub integrating_branches: usize,
    pub merged: usize,
    pub conflicted: usize,
    pub integration_branch: Option<String>,
}

impl SwarmModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset to idle/empty, then seed one `Pending` node per sub-task.
    pub fn decompose(&mut self, tasks: &[(String, String)]) {
        *self = Self::default();
        self.phase = Phase::Fanning;
        self.nodes = tasks
            .iter()
            .enumerate()
            .map(|(index, (role, task))| SwarmNode {
                index,
                role: role.clone(),
                task: task.clone(),
                status: NodeStatus::Pending,
                committed: false,
            })
            .collect();
    }

    /// A fan-out is on screen (fanning out or integrating).
    pub fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Fanning | Phase::Integrating)
    }

    /// Mark node `index` as running. If it wasn't seeded (a `CoderStarted`
    /// without a preceding `Decomposed`), add it — the swarm should never drop
    /// an agent that actually ran.
    pub fn coder_started(&mut self, index: usize, role: &str, task: &str) {
        match self.nodes.iter_mut().find(|n| n.index == index) {
            Some(node) => node.status = NodeStatus::Running,
            None => self.nodes.push(SwarmNode {
                index,
                role: role.to_string(),
                task: task.to_string(),
                status: NodeStatus::Running,
                committed: false,
            }),
        }
        if self.phase == Phase::Idle {
            self.phase = Phase::Fanning;
        }
    }

    /// Mark node `index` finished. `status` is the fan-out's human summary
    /// (e.g. "verified", "verify failed", "no changes"); a summary containing
    /// "fail" → `Failed`, otherwise `Done`.
    pub fn coder_finished(&mut self, index: usize, committed: bool, status: &str) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.index == index) {
            node.committed = committed;
            node.status = if status.to_ascii_lowercase().contains("fail") {
                NodeStatus::Failed
            } else {
                NodeStatus::Done
            };
        }
    }

    /// Enter the integrate phase.
    pub fn integrating(&mut self, branches: usize) {
        self.integrating_branches = branches;
        self.phase = Phase::Integrating;
    }

    /// Fan-out finished — record the integration outcome.
    pub fn done(&mut self, integration_branch: Option<String>, merged: usize, conflicted: usize) {
        self.phase = Phase::Done;
        self.integration_branch = integration_branch;
        self.merged = merged;
        self.conflicted = conflicted;
    }

    pub fn running(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Running)
            .count()
    }
    pub fn done_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Done)
            .count()
    }
    pub fn failed_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Failed)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompose_seeds_pending_nodes() {
        let mut m = SwarmModel::new();
        m.decompose(&[
            ("coder".into(), "add retry".into()),
            ("test".into(), "cover it".into()),
        ]);
        assert_eq!(m.nodes.len(), 2);
        assert!(m.nodes.iter().all(|n| n.status == NodeStatus::Pending));
        assert_eq!(m.nodes[0].role, "coder");
        assert_eq!(m.nodes[1].index, 1);
        assert!(m.is_active(), "fan-out is active after decompose");
    }

    #[test]
    fn coder_started_marks_running() {
        let mut m = SwarmModel::new();
        m.decompose(&[("coder".into(), "t".into())]);
        m.coder_started(0, "coder", "t");
        assert_eq!(m.nodes[0].status, NodeStatus::Running);
        assert_eq!(m.running(), 1);
    }

    #[test]
    fn coder_finished_marks_done_or_failed_from_status() {
        let mut m = SwarmModel::new();
        m.decompose(&[("a".into(), "t".into()), ("b".into(), "t".into())]);
        m.coder_finished(0, true, "verified");
        m.coder_finished(1, false, "verify failed");
        assert_eq!(m.nodes[0].status, NodeStatus::Done);
        assert!(m.nodes[0].committed);
        assert_eq!(m.nodes[1].status, NodeStatus::Failed);
        assert_eq!(m.done_count(), 1);
        assert_eq!(m.failed_count(), 1);
    }

    #[test]
    fn started_for_unknown_index_adds_a_node() {
        let mut m = SwarmModel::new();
        m.coder_started(3, "coder", "t");
        assert_eq!(m.nodes.len(), 1);
        assert_eq!(m.nodes[0].index, 3);
        assert_eq!(m.nodes[0].status, NodeStatus::Running);
    }

    #[test]
    fn integrating_then_done_sets_phase_and_totals() {
        let mut m = SwarmModel::new();
        m.decompose(&[("a".into(), "t".into())]);
        m.integrating(1);
        assert_eq!(m.phase, Phase::Integrating);
        assert!(m.is_active());
        m.done(Some("entheai/fanout-x".into()), 1, 0);
        assert_eq!(m.phase, Phase::Done);
        assert!(!m.is_active(), "done runs are no longer active");
        assert_eq!(m.merged, 1);
        assert_eq!(m.integration_branch.as_deref(), Some("entheai/fanout-x"));
    }
}
