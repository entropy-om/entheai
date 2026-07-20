//! Wire DTOs + object-store key helpers for the F2 work-queue.
use serde::{Deserialize, Serialize};

/// A unit of coder work enqueued on `entheai.work.coder`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkItem {
    pub session: String,
    pub index: usize,
    pub role: String,
    pub task: String,
    /// Object-store key of the base repo bundle the worker must materialize.
    pub base_bundle_key: String,
    /// The commit the bundle checks out to (worker branches from here).
    pub base_sha: String,
}

/// A worker's outcome, published to `entheai.result.<session>.<index>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkResult {
    pub session: String,
    pub index: usize,
    /// "committed" | "no-change" | "error".
    pub status: String,
    pub committed: bool,
    /// Object-store key of the delta bundle (empty when nothing changed).
    pub result_bundle_key: String,
    /// The coder's captured output/log (truncated).
    pub log: String,
}

/// Core-NATS subject a worker publishes its result on / the dispatcher awaits.
pub fn result_subject(session: &str, index: usize) -> String {
    format!("entheai.result.{session}.{index}")
}
/// Object-store key for a session's base bundle.
pub fn base_key(session: &str, index: usize) -> String {
    format!("base/{session}/{index}")
}
/// Object-store key for a session/index's result delta bundle.
pub fn result_key(session: &str, index: usize) -> String {
    format!("result/{session}/{index}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_and_keys_are_stable() {
        assert_eq!(result_subject("abc", 2), "entheai.result.abc.2");
        assert_eq!(base_key("abc", 0), "base/abc/0");
        assert_eq!(result_key("abc", 1), "result/abc/1");
    }

    #[test]
    fn work_item_json_round_trips() {
        let w = WorkItem { session: "s".into(), index: 1, role: "coder".into(), task: "t".into(), base_bundle_key: base_key("s", 1), base_sha: "deadbeef".into() };
        let j = serde_json::to_vec(&w).unwrap();
        assert_eq!(serde_json::from_slice::<WorkItem>(&j).unwrap(), w);
    }

    #[test]
    fn work_result_json_round_trips() {
        let r = WorkResult { session: "s".into(), index: 1, status: "committed".into(), committed: true, result_bundle_key: result_key("s", 1), log: "ok".into() };
        let j = serde_json::to_vec(&r).unwrap();
        assert_eq!(serde_json::from_slice::<WorkResult>(&j).unwrap(), r);
    }
}
