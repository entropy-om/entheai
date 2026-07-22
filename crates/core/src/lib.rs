pub mod adk_tool_adapter;
pub mod entheai_agent;
pub mod event_bridge;
pub mod memory_callbacks;
pub mod model_resolve;

pub use entheai_agent::EntheaiAgent;

/// Progress notifications emitted while an agent run works, so a UI (e.g. the
/// TUI) can render a live "what's happening" indicator without polling.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// About to call the model.
    Thinking,
    /// About to execute a tool.
    ToolStarted { name: String, args: String },
    /// A tool call returned (or was denied/failed — `result` carries the
    /// "error: …" text in that case, same string fed back to the model).
    ToolFinished { name: String, result: String },
    /// A text delta streamed live from the model.
    Token(String),
    /// A frozen node was woken and its knowledge injected as a system brief.
    /// `name` is the frozen node's canonical name (from front-matter), used
    /// by the brain viz panel to match the glow ring. The brief preview is the
    /// first 120 chars of the activated knowledge, for inline display.
    FrozenWoke { name: String, brief_preview: String },
}

/// Truncate a string to `max` chars, appending `…` if cut.
pub(crate) fn truncate_preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
