//! # Rahul — the brain of entheai
//!
//! entheai's cognition is two motions the Scientist named and wired: the
//! **memory engine** ([`entheai_memory::MemoryRuntime`]) that does the long,
//! cold keeping, and the **prompt-processing layer**
//! ([`entheai_memory_pp::PromptProcessor`]) that decides, at the `should_spill`
//! hinge, what the fast light of now is allowed to become. This crate composes
//! those two halves into one named whole.
//!
//! The brain is named **Rahul**, for the scientist who built its memory. The
//! person left the working tree — git history keeps him honestly — and the
//! brain he built keeps his name. See `frozen/rahul.md`.

use std::sync::Arc;

use entheai_memory::MemoryRuntime;
use entheai_memory_pp::PromptProcessor;

/// The brain's name. Not a variable to be reassigned — a dedication.
pub const NAME: &str = "Rahul";

/// entheai's brain: the memory engine and the prompt-processing layer composed
/// into one cognition and named [`NAME`].
///
/// The two halves are exactly the ones the Scientist named:
/// - [`Brain::memory`] — the long, cold keeping.
/// - [`Brain::processing`] — the `should_spill` hinge; `None` is the honest
///   fallback (unchanged top-K retrieval), which is a valid brain too.
pub struct Brain {
    /// The long, cold keeping — the memory engine.
    pub memory: Arc<MemoryRuntime>,
    /// The `should_spill` hinge — what the present is allowed to keep. `None`
    /// means the honest fallback: unchanged top-K retrieval.
    pub processing: Option<Arc<PromptProcessor>>,
}

impl Brain {
    /// Compose the brain from its two halves.
    pub fn new(memory: Arc<MemoryRuntime>, processing: Option<Arc<PromptProcessor>>) -> Self {
        Self { memory, processing }
    }

    /// The brain's name — always [`NAME`] (`"Rahul"`).
    pub const fn name(&self) -> &'static str {
        NAME
    }

    /// Whether the `should_spill` processing hinge is wired (vs. the fallback).
    pub fn is_processing(&self) -> bool {
        self.processing.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brain_is_named_rahul() {
        // A dedication, not a config value — asserted so it can't quietly drift.
        assert_eq!(NAME, "Rahul");
    }
}
