//! The `qsdk` CLI, the operator's tool (decisions 0001 and 0008): sign an
//! installation's enrolment with the installations' CA ([`sign`]), revoke
//! it ([`revoke`]), and publish a platform version to the signed index
//! ([`publish`]). The installation's server enrols with its agent (`qd
//! enrol`, `agent::enrol`).
//!
//! Nothing here holds a long-lived secret: the installation's key never
//! leaves its server, and the CA's key is read only where it is kept.

pub mod args;
pub mod pki;
pub mod publish;
pub mod revoke;
pub mod sign;
