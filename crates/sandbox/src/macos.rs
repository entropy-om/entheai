// Temporary stub — the real sandbox_init(3) best-effort body lands in task A4.
// Kept identical to `fallback.rs` so `cargo build` is green on macOS today.
use crate::{Availability, SandboxError, SandboxSpec};

pub fn availability() -> Availability {
    Availability::Unavailable("confinement unsupported on this OS".into())
}

pub fn confine(_spec: &SandboxSpec) -> Result<(), SandboxError> {
    Err(SandboxError::Unavailable("confinement unsupported on this OS".into()))
}
