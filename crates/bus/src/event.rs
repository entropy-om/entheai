//! Wire DTO for fan-out events. Mirrors `orchestrator::FanoutEvent` so the
//! `orchestrator` crate needs no serde-for-wire dependency, and owns the
//! subject-suffix + JSON contract that tailnet subscribers depend on.

use entheai_orchestrator::FanoutEvent;
use serde::Serialize;

/// JSON-serializable mirror of `FanoutEvent`, tagged by `event` kind. Published
/// to `entheai.fanout.<session>.<subject_suffix()>`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BusEvent {
    Fallback,
    Decomposed { tasks: Vec<(String, String)> },
    CoderStarted { index: usize, role: String, task: String },
    CoderFinished { index: usize, committed: bool, status: String },
    Integrating { branches: usize },
    Done { integration_branch: Option<String>, merged: usize, conflicted: usize },
}

impl BusEvent {
    /// Dotted subject suffix (under `entheai.fanout.<session>.`) — matches the
    /// taxonomy in the federation design spec §2.
    pub fn subject_suffix(&self) -> &'static str {
        match self {
            BusEvent::Fallback => "fallback",
            BusEvent::Decomposed { .. } => "decomposed",
            BusEvent::CoderStarted { .. } => "coder.started",
            BusEvent::CoderFinished { .. } => "coder.finished",
            BusEvent::Integrating { .. } => "integrating",
            BusEvent::Done { .. } => "done",
        }
    }
}

impl From<&FanoutEvent> for BusEvent {
    fn from(e: &FanoutEvent) -> Self {
        match e {
            FanoutEvent::Fallback => BusEvent::Fallback,
            FanoutEvent::Decomposed { tasks } => BusEvent::Decomposed { tasks: tasks.clone() },
            FanoutEvent::CoderStarted { index, role, task } => BusEvent::CoderStarted {
                index: *index,
                role: role.clone(),
                task: task.clone(),
            },
            FanoutEvent::CoderFinished { index, committed, status } => BusEvent::CoderFinished {
                index: *index,
                committed: *committed,
                status: status.clone(),
            },
            FanoutEvent::Integrating { branches } => BusEvent::Integrating { branches: *branches },
            FanoutEvent::Done { integration_branch, merged, conflicted } => BusEvent::Done {
                integration_branch: integration_branch.clone(),
                merged: *merged,
                conflicted: *conflicted,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_a_distinct_subject_suffix() {
        let suffixes = [
            BusEvent::Fallback.subject_suffix(),
            BusEvent::Decomposed { tasks: vec![] }.subject_suffix(),
            BusEvent::CoderStarted { index: 0, role: String::new(), task: String::new() }.subject_suffix(),
            BusEvent::CoderFinished { index: 0, committed: false, status: String::new() }.subject_suffix(),
            BusEvent::Integrating { branches: 0 }.subject_suffix(),
            BusEvent::Done { integration_branch: None, merged: 0, conflicted: 0 }.subject_suffix(),
        ];
        let unique: std::collections::HashSet<_> = suffixes.iter().collect();
        assert_eq!(unique.len(), suffixes.len(), "subject suffixes must be unique");
        assert_eq!(BusEvent::CoderStarted { index: 0, role: String::new(), task: String::new() }.subject_suffix(), "coder.started");
    }

    #[test]
    fn from_fanout_event_preserves_fields() {
        let fe = FanoutEvent::CoderFinished { index: 2, committed: true, status: "verified".into() };
        assert_eq!(
            BusEvent::from(&fe),
            BusEvent::CoderFinished { index: 2, committed: true, status: "verified".into() }
        );
    }

    #[test]
    fn serializes_to_tagged_json() {
        let json = serde_json::to_string(&BusEvent::Integrating { branches: 3 }).unwrap();
        assert_eq!(json, r#"{"event":"integrating","branches":3}"#);
    }

    #[test]
    fn done_serializes_all_fields() {
        let json = serde_json::to_string(&BusEvent::Done {
            integration_branch: Some("fanout/abc".into()),
            merged: 2,
            conflicted: 1,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"event":"done","integration_branch":"fanout/abc","merged":2,"conflicted":1}"#
        );
    }
}
