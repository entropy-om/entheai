//! Structures large prompts/tasks/files into sectioned, chunked input for the
//! orchestrator's fan-out decompose step. Never decides fan-out itself.

mod files;
mod sections;

pub use files::FileChunk;
pub use sections::PromptSection;
