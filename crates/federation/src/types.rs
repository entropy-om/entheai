//! Wire DTOs + object-store key helpers for the F2 work-queue.
use serde::{Deserialize, Serialize};

/// Is `s` a well-formed git object id (40-hex SHA-1 or 64-hex SHA-256)?
/// `WorkItem::base_sha` arrives off the wire and is used both as a filesystem
/// path component on every worker (`<cache>/<base_sha>.git`, followed by
/// `remove_dir_all`) and as a git argument (`<base_sha>..HEAD`), so anything
/// else — `../../..`, `-`-prefixed option injection — must be rejected before use.
pub fn is_valid_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

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
    /// Base-repo outcome tag the orchestrator reads: "hit" (base was cached),
    /// "miss" (materialized fresh into the cache), or "degraded:<reason>" (the
    /// shared-base fast path failed and the worker fell back to a full clone).
    /// `#[serde(default)]` keeps older workers' results (no `base`) deserializable.
    #[serde(default)]
    pub base: String,
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
    fn is_valid_sha_accepts_object_ids_only() {
        assert!(is_valid_sha(&"a".repeat(40)));
        assert!(is_valid_sha(&"0123456789abcdef".repeat(4)));
        assert!(!is_valid_sha(""));
        assert!(!is_valid_sha("HEAD"));
        assert!(!is_valid_sha("../../../home/u/proj"));
        assert!(!is_valid_sha(&format!("-{}", "a".repeat(39))));
        assert!(!is_valid_sha(&"g".repeat(40)));
        assert!(!is_valid_sha(&"a".repeat(41)));
    }

    #[test]
    fn subject_and_keys_are_stable() {
        assert_eq!(result_subject("abc", 2), "entheai.result.abc.2");
        assert_eq!(base_key("abc", 0), "base/abc/0");
        assert_eq!(result_key("abc", 1), "result/abc/1");
    }

    #[test]
    fn work_item_json_round_trips() {
        let w = WorkItem {
            session: "s".into(),
            index: 1,
            role: "coder".into(),
            task: "t".into(),
            base_bundle_key: base_key("s", 1),
            base_sha: "deadbeef".into(),
        };
        let j = serde_json::to_vec(&w).unwrap();
        assert_eq!(serde_json::from_slice::<WorkItem>(&j).unwrap(), w);
    }

    #[test]
    fn work_result_json_round_trips() {
        let r = WorkResult {
            session: "s".into(),
            index: 1,
            status: "committed".into(),
            committed: true,
            result_bundle_key: result_key("s", 1),
            log: "ok".into(),
            base: "hit".into(),
        };
        let j = serde_json::to_vec(&r).unwrap();
        assert_eq!(serde_json::from_slice::<WorkResult>(&j).unwrap(), r);
    }

    #[test]
    fn work_result_base_defaults_when_absent() {
        // An older worker's result (no `base` field) must still deserialize.
        let legacy = r#"{"session":"s","index":1,"status":"committed","committed":true,"result_bundle_key":"result/s/1","log":"ok"}"#;
        let r: WorkResult = serde_json::from_str(legacy).unwrap();
        assert_eq!(r.base, "");
    }
}
