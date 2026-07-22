//! Portable process-confinement for the federation worker's coder child.
//! `confine()` is called by the `entheai-worker --sandbox-run` child at startup,
//! before any model/tool code runs; it is irreversible for the calling process.

use std::path::PathBuf;

/// Confinement posture, from `[federation] sandbox`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    /// Refuse to run the coder if confinement can't be applied.
    Strict,
    /// Attempt confinement; if unavailable, warn and run unconfined (default).
    #[default]
    Permissive,
    /// Never attempt confinement (today's behavior).
    Off,
}

impl SandboxMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "strict" => Some(Self::Strict),
            "permissive" => Some(Self::Permissive),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// What the child asks `confine()` to enforce.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    /// The one writable+executable directory (the coder's worktree).
    pub work_dir: PathBuf,
    /// Paths granted read+execute (toolchain, CA certs, config, …).
    pub read_only_paths: Vec<PathBuf>,
    /// If the process is root, drop to this (uid, gid). `None` = skip.
    pub drop_uid: Option<(u32, u32)>,
}

/// Whether this host can confine at all (cheap probe, no side effects).
#[derive(Debug)]
pub enum Availability {
    Available,
    Unavailable(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("sandbox unavailable on this host: {0}")]
    Unavailable(String),
    #[error("failed to apply sandbox: {0}")]
    Apply(String),
}

/// Probe whether confinement is possible here (kernel/OS support).
pub fn availability() -> Availability {
    imp::availability()
}

/// Apply the sandbox to the CURRENT process/thread. Irreversible. Call once,
/// at child startup, before running any untrusted code.
pub fn confine(spec: &SandboxSpec) -> Result<(), SandboxError> {
    imp::confine(spec)
}

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[path = "fallback.rs"]
mod imp;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mode_parses_and_defaults() {
        assert_eq!(SandboxMode::parse("strict"), Some(SandboxMode::Strict));
        assert_eq!(
            SandboxMode::parse("  Permissive "),
            Some(SandboxMode::Permissive)
        );
        assert_eq!(SandboxMode::parse("off"), Some(SandboxMode::Off));
        assert_eq!(SandboxMode::parse("nope"), None);
        assert_eq!(SandboxMode::default(), SandboxMode::Permissive);
    }
}
