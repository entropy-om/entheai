//! Structures large prompts/tasks/files into sectioned, chunked input for the
//! orchestrator's fan-out decompose step. Never decides fan-out itself.

mod sections;

pub use sections::PromptSection;
