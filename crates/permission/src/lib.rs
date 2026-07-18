#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

/// Static permission policy. Resolution order: yolo → allowlist → ask.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub yolo: bool,
    pub allowlist: Vec<String>,
}

impl Policy {
    pub fn decide(&self, tool_name: &str) -> Decision {
        if self.yolo {
            return Decision::Allow;
        }
        if self.allowlist.iter().any(|t| t == tool_name) {
            return Decision::Allow;
        }
        Decision::Ask
    }
}

/// Resolves an `Ask` decision to a concrete yes/no. Stdin impl for the CLI;
/// tests use their own.
pub trait Prompter {
    /// Return true to allow this tool call.
    fn confirm(&mut self, tool_name: &str, args_summary: &str) -> bool;
}

/// Reads a y/N line from stdin.
pub struct StdinPrompter;
impl Prompter for StdinPrompter {
    fn confirm(&mut self, tool_name: &str, args_summary: &str) -> bool {
        use std::io::Write;
        eprint!("allow {tool_name}({args_summary})? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return false;
        }
        matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yolo_allows_everything() {
        let policy = Policy {
            yolo: true,
            allowlist: vec![],
        };
        assert_eq!(policy.decide("run_shell"), Decision::Allow);
    }

    #[test]
    fn allowlist_allows_named_tool() {
        let policy = Policy {
            yolo: false,
            allowlist: vec!["read_file".into()],
        };
        assert_eq!(policy.decide("read_file"), Decision::Allow);
        assert_eq!(policy.decide("run_shell"), Decision::Ask);
    }

    #[test]
    fn default_is_ask() {
        let policy = Policy {
            yolo: false,
            allowlist: vec![],
        };
        assert_eq!(policy.decide("write_file"), Decision::Ask);
    }
}
