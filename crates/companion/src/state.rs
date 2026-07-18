use serde::{Deserialize, Serialize};

/// Session state pushed over the Unix socket to the companion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Idle,
    Working,
    PermissionPending,
    Error,
}

/// A state-change event sent from the session to the companion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChange {
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl StateChange {
    /// Create a simple state transition with no extra fields.
    pub fn new(state: State) -> Self {
        Self {
            state,
            tool: None,
            args: None,
            message: None,
        }
    }

    /// Working state.
    pub fn working() -> Self {
        Self::new(State::Working)
    }

    /// Idle state.
    pub fn idle() -> Self {
        Self::new(State::Idle)
    }

    /// A tool is waiting for permission approval.
    pub fn permission_pending(tool: impl Into<String>, args: impl Into<String>) -> Self {
        Self {
            state: State::PermissionPending,
            tool: Some(tool.into()),
            args: Some(args.into()),
            message: None,
        }
    }

    /// An error occurred.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            state: State::Error,
            tool: None,
            args: None,
            message: Some(message.into()),
        }
    }
}
