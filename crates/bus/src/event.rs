//! Wire DTO for fan-out events. Mirrors `orchestrator::FanoutEvent` so the
//! `orchestrator` crate needs no serde-for-wire dependency, and owns the
//! subject-suffix + JSON contract that tailnet subscribers depend on.

use entheai_orchestrator::FanoutEvent;
use serde::{Deserialize, Serialize};

/// Schema tag AND subject prefix for entropy telemetry (roadmap Phase 1.2).
/// A breaking layout change bumps the `v1` in both — wire rules per
/// VERSIONING.md.
pub const ENTROPY_SUBJECT_PREFIX: &str = "entheai.entropy.v1";

/// Live TUI entropy telemetry, published to `entheai.entropy.v1.<session>` at
/// ~1 Hz while the TUI runs with the bus on: brain-ring glow intensities,
/// frozen-node wake glows, the pomodoro tick, and the `wk N` worker count.
/// `Deserialize` too — subscribers (the site bridge, tailnet dashboards)
/// decode with the same type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntropySnapshot {
    /// Always [`ENTROPY_SUBJECT_PREFIX`].
    pub schema: String,
    pub session: String,
    pub at_ms: i64,
    /// (faculty name, glow 0..=1) — the brain-ring intensities.
    pub glow: Vec<(String, f32)>,
    /// (frozen node name, wake glow 0..=1) for currently-loaded nodes.
    pub frozen: Vec<(String, f32)>,
    /// Seconds elapsed in the always-on pomodoro.
    pub pomodoro_sec: u64,
    /// Live `wk N` worker count.
    pub workers: usize,
    /// Last compression cycle's output/input ratio, when one has run.
    pub compression_ratio: Option<f64>,
}

/// JSON-serializable mirror of `FanoutEvent`, tagged by `event` kind. Published
/// to `entheai.fanout.<session>.<subject_suffix()>`. The `event` tag is kept
/// identical to `subject_suffix()` so an event kind has exactly ONE canonical
/// string on the wire (the two multi-word kinds carry an explicit dotted
/// `rename` to match their dotted NATS subject — see the `event_tag_matches_*`
/// test). `snake_case` covers the single-word variants.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BusEvent {
    Fallback,
    Decomposed {
        tasks: Vec<(String, String)>,
    },
    #[serde(rename = "coder.started")]
    CoderStarted {
        index: usize,
        role: String,
        task: String,
    },
    #[serde(rename = "coder.finished")]
    CoderFinished {
        index: usize,
        committed: bool,
        status: String,
    },
    Integrating {
        branches: usize,
    },
    Done {
        integration_branch: Option<String>,
        merged: usize,
        conflicted: usize,
    },
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
            FanoutEvent::Decomposed { tasks } => BusEvent::Decomposed {
                tasks: tasks.clone(),
            },
            FanoutEvent::CoderStarted { index, role, task } => BusEvent::CoderStarted {
                index: *index,
                role: role.clone(),
                task: task.clone(),
            },
            FanoutEvent::CoderFinished {
                index,
                committed,
                status,
            } => BusEvent::CoderFinished {
                index: *index,
                committed: *committed,
                status: status.clone(),
            },
            FanoutEvent::Integrating { branches } => BusEvent::Integrating {
                branches: *branches,
            },
            FanoutEvent::Done {
                integration_branch,
                merged,
                conflicted,
            } => BusEvent::Done {
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
    fn entropy_snapshot_round_trips_and_carries_the_schema_tag() {
        let snap = EntropySnapshot {
            schema: ENTROPY_SUBJECT_PREFIX.to_string(),
            session: "sess-1".into(),
            at_ms: 1_753_000_000_000,
            glow: vec![("Model".into(), 0.8), ("Tools".into(), 0.1)],
            frozen: vec![("verification".into(), 1.0)],
            pomodoro_sec: 300,
            workers: 4,
            compression_ratio: Some(0.42),
        };
        let json = serde_json::to_string(&snap).unwrap();
        // The wire contract subscribers grep for.
        assert!(json.contains("\"schema\":\"entheai.entropy.v1\""));
        assert!(json.contains("\"pomodoro_sec\":300"));
        assert!(json.contains("\"workers\":4"));
        let back: EntropySnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snap, "2-way wire serialization is lossless");
    }

    #[test]
    fn every_variant_has_a_distinct_subject_suffix() {
        let suffixes = [
            BusEvent::Fallback.subject_suffix(),
            BusEvent::Decomposed { tasks: vec![] }.subject_suffix(),
            BusEvent::CoderStarted {
                index: 0,
                role: String::new(),
                task: String::new(),
            }
            .subject_suffix(),
            BusEvent::CoderFinished {
                index: 0,
                committed: false,
                status: String::new(),
            }
            .subject_suffix(),
            BusEvent::Integrating { branches: 0 }.subject_suffix(),
            BusEvent::Done {
                integration_branch: None,
                merged: 0,
                conflicted: 0,
            }
            .subject_suffix(),
        ];
        let unique: std::collections::HashSet<_> = suffixes.iter().collect();
        assert_eq!(
            unique.len(),
            suffixes.len(),
            "subject suffixes must be unique"
        );
        assert_eq!(
            BusEvent::CoderStarted {
                index: 0,
                role: String::new(),
                task: String::new()
            }
            .subject_suffix(),
            "coder.started"
        );
    }

    #[test]
    fn event_tag_matches_subject_suffix_for_every_variant() {
        // One canonical string per event kind: the JSON `event` tag MUST equal
        // the subject suffix, so a subscriber filtering by subject and a consumer
        // parsing the body never see two different names for the same event.
        let all = [
            BusEvent::Fallback,
            BusEvent::Decomposed { tasks: vec![] },
            BusEvent::CoderStarted {
                index: 0,
                role: String::new(),
                task: String::new(),
            },
            BusEvent::CoderFinished {
                index: 0,
                committed: false,
                status: String::new(),
            },
            BusEvent::Integrating { branches: 0 },
            BusEvent::Done {
                integration_branch: None,
                merged: 0,
                conflicted: 0,
            },
        ];
        for ev in &all {
            let json: serde_json::Value = serde_json::to_value(ev).unwrap();
            assert_eq!(
                json["event"].as_str().unwrap(),
                ev.subject_suffix(),
                "JSON event tag must equal subject suffix for {ev:?}"
            );
        }
    }

    #[test]
    fn from_fanout_event_preserves_fields_for_every_variant() {
        let cases = [
            (FanoutEvent::Fallback, BusEvent::Fallback),
            (
                FanoutEvent::Decomposed {
                    tasks: vec![("coder".into(), "t".into())],
                },
                BusEvent::Decomposed {
                    tasks: vec![("coder".into(), "t".into())],
                },
            ),
            (
                FanoutEvent::CoderStarted {
                    index: 1,
                    role: "explore".into(),
                    task: "look".into(),
                },
                BusEvent::CoderStarted {
                    index: 1,
                    role: "explore".into(),
                    task: "look".into(),
                },
            ),
            (
                FanoutEvent::CoderFinished {
                    index: 2,
                    committed: true,
                    status: "verified".into(),
                },
                BusEvent::CoderFinished {
                    index: 2,
                    committed: true,
                    status: "verified".into(),
                },
            ),
            (
                FanoutEvent::Integrating { branches: 3 },
                BusEvent::Integrating { branches: 3 },
            ),
            (
                FanoutEvent::Done {
                    integration_branch: Some("b".into()),
                    merged: 2,
                    conflicted: 1,
                },
                BusEvent::Done {
                    integration_branch: Some("b".into()),
                    merged: 2,
                    conflicted: 1,
                },
            ),
        ];
        for (fe, expected) in &cases {
            assert_eq!(&BusEvent::from(fe), expected, "mapping mismatch for {fe:?}");
        }
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
