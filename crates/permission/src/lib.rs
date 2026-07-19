#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Permission policy. Resolution order: yolo → allowlist → session grants → ask.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub yolo: bool,
    pub allowlist: Vec<String>,
    /// Tools granted "for this session" at runtime (via `Grant::AllowSession`).
    session: Arc<Mutex<HashSet<String>>>,
}

impl Policy {
    pub fn new(yolo: bool, allowlist: Vec<String>) -> Self {
        Self {
            yolo,
            allowlist,
            session: Arc::new(Mutex::new(HashSet::new())),
        }
    }
    /// Grant a tool for the rest of the session; subsequent `decide` calls Allow it.
    pub fn grant_session(&self, tool_name: &str) {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(tool_name.to_string());
    }
    pub fn decide(&self, tool_name: &str) -> Decision {
        if self.yolo || self.allowlist.iter().any(|t| t == tool_name) {
            return Decision::Allow;
        }
        if self
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(tool_name)
        {
            return Decision::Allow;
        }
        Decision::Ask
    }
}

/// A user's answer to a permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// Deny this call.
    Deny,
    /// Allow just this one call.
    Allow,
    /// Allow this tool for the rest of the session (no more prompts for it).
    AllowSession,
}

/// Resolves an `Ask` decision to a concrete yes/no/session-yes. Stdin impl for
/// the CLI; the TUI supplies a modal-backed impl. Async so a UI can await user
/// input without blocking the runtime. `Send` so `run_task` stays spawnable.
#[async_trait::async_trait]
pub trait Prompter: Send {
    /// Return the user's answer to allow/deny this tool call.
    async fn confirm(&mut self, tool_name: &str, args_summary: &str) -> Grant;
}

/// Reads a y/n/a line from stdin. The blocking read runs on a dedicated thread
/// so it never stalls the async runtime.
pub struct StdinPrompter;

#[async_trait::async_trait]
impl Prompter for StdinPrompter {
    async fn confirm(&mut self, tool_name: &str, args_summary: &str) -> Grant {
        use std::io::Write;
        eprint!("allow {tool_name}({args_summary})? [y]es / [n]o / [a]llow for session ");
        let _ = std::io::stderr().flush();
        tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_err() {
                return Grant::Deny;
            }
            match line.trim().to_lowercase().as_str() {
                "y" | "yes" => Grant::Allow,
                "a" | "allow" | "s" | "session" => Grant::AllowSession,
                _ => Grant::Deny,
            }
        })
        .await
        .unwrap_or(Grant::Deny)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yolo_allows_everything() {
        let policy = Policy::new(true, vec![]);
        assert_eq!(policy.decide("run_shell"), Decision::Allow);
    }

    #[test]
    fn allowlist_allows_named_tool() {
        let policy = Policy::new(false, vec!["read_file".into()]);
        assert_eq!(policy.decide("read_file"), Decision::Allow);
        assert_eq!(policy.decide("run_shell"), Decision::Ask);
    }

    #[test]
    fn default_is_ask() {
        let policy = Policy::new(false, vec![]);
        assert_eq!(policy.decide("write_file"), Decision::Ask);
    }

    #[test]
    fn grant_has_three_variants() {
        let _ = (Grant::Deny, Grant::Allow, Grant::AllowSession);
        assert_ne!(Grant::Allow, Grant::Deny);
    }

    #[test]
    fn session_grant_makes_decide_allow() {
        let p = Policy::new(false, vec![]);
        assert_eq!(p.decide("run_shell"), Decision::Ask);
        p.grant_session("run_shell");
        assert_eq!(p.decide("run_shell"), Decision::Allow);
        assert_eq!(p.decide("write_file"), Decision::Ask); // unaffected
    }
}
