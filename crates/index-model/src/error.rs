//! Why an index is refused.

/// An index the agent must not act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IndexError {
    /// The public key is not a P-256 key in SPKI DER or PEM.
    #[error("the index public key is not valid: {0}")]
    BadKey(String),
    /// The signature is not base64 of a DER ECDSA signature.
    #[error("the index signature is malformed: {0}")]
    MalformedSignature(String),
    /// The signature does not match the index and the pinned key.
    #[error("the index signature does not verify")]
    BadSignature,
    /// The index is not the JSON this crate reads.
    #[error("the index is malformed: {0}")]
    Malformed(String),
    /// A format this agent does not know: it needs a newer agent.
    #[error("index format {0} is not supported by this agent")]
    UnsupportedFormat(u32),
    /// The index parses but breaks a rule (a tag instead of a digest, two
    /// entries for one version, a percentage over 100…).
    #[error("the index is invalid: {0}")]
    Invalid(String),
    /// An older index than the last one accepted: a replay.
    #[error("index sequence {got} is older than {seen}, the last accepted")]
    Replay { seen: u64, got: u64 },
}
