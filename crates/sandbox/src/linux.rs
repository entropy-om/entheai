// Temporary stub — the real Landlock/seccomp/uid-drop body lands in task A3.
// Kept identical to `fallback.rs` so `cargo build` is green on Linux today.
use crate::{Availability, SandboxError, SandboxSpec};

pub fn availability() -> Availability {
    Availability::Unavailable("confinement unsupported on this OS".into())
}

pub fn confine(_spec: &SandboxSpec) -> Result<(), SandboxError> {
    Err(SandboxError::Unavailable("confinement unsupported on this OS".into()))
}
