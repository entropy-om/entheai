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
}
