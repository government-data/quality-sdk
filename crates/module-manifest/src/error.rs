//! Why a manifest is refused.

/// A manifest the agent must not act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// Not the YAML this crate reads (a key it does not know, a wrong
    /// type, a missing field).
    #[error("the module manifest is malformed: {0}")]
    Malformed(String),
    /// It reads but breaks a rule (a name that cannot be used, a
    /// capability declared twice, a path outside the module…).
    #[error("the module manifest is invalid: {0}")]
    Invalid(String),
}
