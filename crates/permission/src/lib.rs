#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

/// Tool risk tier, ordered by how much autonomy the tool exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Read,
    Write,
    Exec,
    Network,
    Spawn,
}

/// Runtime permission posture, cycled with Shift+Tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    Plan,
    Auto,
    Yolo,
    #[default]
    Ask,
}

impl Mode {
    /// Shift+Tab order: plan → auto → yolo → ask → plan.
    pub fn next(self) -> Mode {
        match self {
            Mode::Plan => Mode::Auto,
            Mode::Auto => Mode::Yolo,
            Mode::Yolo => Mode::Ask,
            Mode::Ask => Mode::Plan,
        }
    }

    /// The highest tier an unattended subagent auto-approves under this mode.
    pub fn ceiling(self) -> Tier {
        match self {
            Mode::Plan => Tier::Read,
            Mode::Auto | Mode::Ask => Tier::Exec,
            Mode::Yolo => Tier::Spawn,
        }
    }

    /// Parse the config string; unknown values warn and fall back to `Ask`.
    pub fn parse(s: &str) -> Mode {
        match s.trim().to_ascii_lowercase().as_str() {
            "plan" => Mode::Plan,
            "auto" => Mode::Auto,
            "yolo" => Mode::Yolo,
            "ask" | "" => Mode::Ask,
            other => {
                log::warn!("unknown permission mode {other:?}; defaulting to ask");
                Mode::Ask
            }
        }
    }
}

/// A per-tool override that wins over the mode×tier matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pin {
    AlwaysAllow,
    AlwaysAsk,
    Never, // always Deny
}

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Permission policy. Resolution order: yolo → allowlist → session grants → ask.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub yolo: bool,
    pub allowlist: Vec<String>,
    /// Tools granted "for this session" at runtime (via `Grant::AllowSession`).
    session: Arc<Mutex<HashSet<String>>>,
    mode: Arc<Mutex<Mode>>,
    pins: HashMap<String, Pin>,
    ceiling: Option<Tier>,
}

impl Policy {
    pub fn new(yolo: bool, allowlist: Vec<String>) -> Self {
        let mode = if yolo { Mode::Yolo } else { Mode::Ask };
        Self {
            yolo,
            allowlist,
            session: Arc::new(Mutex::new(HashSet::new())),
            mode: Arc::new(Mutex::new(mode)),
            pins: HashMap::new(),
            ceiling: None,
        }
    }

    pub fn with_mode(mode: Mode) -> Self {
        let p = Policy::new(false, Vec::new());
        p.set_mode(mode);
        p
    }

    pub fn with_ceiling(ceiling: Tier) -> Self {
        Self {
            yolo: false,
            allowlist: Vec::new(),
            session: Arc::new(Mutex::new(HashSet::new())),
            mode: Arc::new(Mutex::new(Mode::Ask)),
            pins: HashMap::new(),
            ceiling: Some(ceiling),
        }
    }

    pub fn mode(&self) -> Mode {
        *self.mode.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_mode(&self, mode: Mode) {
        *self.mode.lock().unwrap_or_else(|e| e.into_inner()) = mode;
    }

    pub fn pin(&mut self, tool: &str, pin: Pin) {
        self.pins.insert(tool.to_string(), pin);
    }

    /// Whether this policy auto-approves everything (yolo mode).
    pub fn is_yolo(&self) -> bool {
        self.mode() == Mode::Yolo || self.yolo
    }

    /// Grant a tool for the rest of the session; subsequent `decide` calls Allow it.
    pub fn grant_session(&self, tool_name: &str) {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(tool_name.to_string());
    }

    /// Tiered decision: pin first, else allowlist/session, else ceiling if present, else the mode×tier matrix.
    pub fn decide_tiered(&self, tool: &str, tier: Tier) -> Decision {
        if let Some(pin) = self.pins.get(tool) {
            return match pin {
                Pin::AlwaysAllow => Decision::Allow,
                Pin::AlwaysAsk => Decision::Ask,
                Pin::Never => Decision::Deny,
            };
        }
        if self.allowlist.iter().any(|t| t == tool)
            || self
                .session
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(tool)
        {
            return Decision::Allow;
        }
        if let Some(ceiling) = self.ceiling {
            return if tier <= ceiling {
                Decision::Allow
            } else {
                Decision::Deny
            };
        }
        match (self.mode(), tier) {
            (Mode::Yolo, _) => Decision::Allow,
            (_, Tier::Read) => Decision::Allow,
            (Mode::Plan, _) => Decision::Deny,
            (Mode::Auto, Tier::Write) => Decision::Allow,
            (Mode::Auto, _) => Decision::Ask,
            (Mode::Ask, _) => Decision::Ask,
        }
    }

    pub fn decide(&self, tool_name: &str) -> Decision {
        self.decide_tiered(tool_name, Tier::Exec)
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

    #[test]
    fn tier_orders_by_autonomy() {
        assert!(Tier::Read < Tier::Write);
        assert!(Tier::Write < Tier::Exec);
        assert!(Tier::Exec < Tier::Network);
        assert!(Tier::Network < Tier::Spawn);
    }

    #[test]
    fn mode_cycles_and_maps_to_ceiling() {
        assert_eq!(Mode::Plan.next(), Mode::Auto);
        assert_eq!(Mode::Auto.next(), Mode::Yolo);
        assert_eq!(Mode::Yolo.next(), Mode::Ask);
        assert_eq!(Mode::Ask.next(), Mode::Plan);
        // subagent ceiling: highest auto-approved tier for an unattended child
        assert_eq!(Mode::Plan.ceiling(), Tier::Read);
        assert_eq!(Mode::Auto.ceiling(), Tier::Exec);
        assert_eq!(Mode::Ask.ceiling(), Tier::Exec);
        assert_eq!(Mode::Yolo.ceiling(), Tier::Spawn);
    }

    #[test]
    fn mode_parse_is_fail_safe() {
        assert_eq!(Mode::parse("plan"), Mode::Plan);
        assert_eq!(Mode::parse("YOLO"), Mode::Yolo);
        assert_eq!(Mode::parse("bogus"), Mode::Ask, "unknown → ask (safe default)");
    }

    #[test]
    fn matrix_matches_the_spec() {
        use Decision::*;
        use Tier::*;
        let cases = [
            (Mode::Plan, [Allow, Deny, Deny, Deny, Deny]),
            (Mode::Auto, [Allow, Allow, Ask, Ask, Ask]),
            (Mode::Yolo, [Allow, Allow, Allow, Allow, Allow]),
            (Mode::Ask,  [Allow, Ask, Ask, Ask, Ask]),
        ];
        let tiers = [Read, Write, Exec, Network, Spawn];
        for (mode, row) in cases {
            let p = Policy::with_mode(mode);
            for (t, want) in tiers.iter().zip(row) {
                assert_eq!(p.decide_tiered("some_tool", *t), want, "{mode:?} × {t:?}");
            }
        }
    }

    #[test]
    fn pins_override_the_matrix() {
        let mut p = Policy::with_mode(Mode::Yolo); // matrix would Allow everything
        p.pin("run_shell", Pin::AlwaysAsk);
        p.pin("rm", Pin::Never);
        p.pin("read_file", Pin::AlwaysAllow);
        assert_eq!(p.decide_tiered("run_shell", Tier::Exec), Decision::Ask);
        assert_eq!(p.decide_tiered("rm", Tier::Exec), Decision::Deny);
        assert_eq!(p.decide_tiered("read_file", Tier::Read), Decision::Allow);
    }

    #[test]
    fn decide_shim_preserves_legacy_semantics() {
        // yolo policy → Allow (legacy); allowlist → Allow; else Ask (Exec-tier default).
        let yolo = Policy::new(true, vec![]);
        assert_eq!(yolo.decide("anything"), Decision::Allow);
        let allow = Policy::new(false, vec!["echo".into()]);
        assert_eq!(allow.decide("echo"), Decision::Allow);
        assert_eq!(allow.decide("rm"), Decision::Ask);
    }
}
