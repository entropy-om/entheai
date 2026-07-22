//! entheai-memory-pp — the opt-in prompt-processing retrieval pipeline.
//!
//! Keep the past RAW; search the raw space; compress LAST. This crate owns the
//! raw experiential store, the mesh/compressor subprocess seams (stubbed in
//! Slice 1), and the orchestrator. It depends on `entheai-memory` for shared
//! types (`MemoryScope`, `ToolEvidence`); `entheai-memory` never depends on it.
//! See docs/superpowers/specs/2026-07-22-prompt-processing-design.md.

mod error;
mod raw_store;

pub use error::PpError;
pub use raw_store::{RawContent, RawKind, RawSpan, RawStore};

/// Which retrieval implementation `run_task_with_memory` dispatches to.
/// `TopK` is today's behaviour; the default guarantees "off unless set".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetrievalMode {
    #[default]
    TopK,
    PromptProcessing,
}

impl RetrievalMode {
    /// Parse the `[memory] mode` config string. Unknown non-"topk" values warn
    /// and default to `TopK` (fail-safe: a typo can never silently disable top-K).
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "prompt-processing" => RetrievalMode::PromptProcessing,
            "topk" | "" => RetrievalMode::TopK,
            other => {
                log::warn!("unknown memory mode {other:?}; defaulting to topk");
                RetrievalMode::TopK
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parse_and_default() {
        assert_eq!(RetrievalMode::default(), RetrievalMode::TopK);
        assert_eq!(RetrievalMode::parse("topk"), RetrievalMode::TopK);
        assert_eq!(RetrievalMode::parse(""), RetrievalMode::TopK);
        assert_eq!(RetrievalMode::parse("prompt-processing"), RetrievalMode::PromptProcessing);
        assert_eq!(RetrievalMode::parse("bogus"), RetrievalMode::TopK);
    }
}
