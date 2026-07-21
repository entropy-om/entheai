use crate::{Availability, SandboxError, SandboxSpec};

pub fn availability() -> Availability {
    Availability::Unavailable("confinement unsupported on this OS".into())
}

pub fn confine(_spec: &SandboxSpec) -> Result<(), SandboxError> {
    Err(SandboxError::Unavailable("confinement unsupported on this OS".into()))
}
