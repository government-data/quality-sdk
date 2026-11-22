//! The signed version index of a channel (decision 0001).
//!
//! The index lists **platform versions**: the core plus the version of each
//! module, tested together, every image pinned by digest and every package by
//! its SHA-256. An agent trusts an index only when its signature verifies with
//! the public key pinned at enrolment ([`verify`]), and only when it is not
//! older than the last one it accepted ([`Index::check_not_older`]). Then
//! [`Index::offer`] says which version this installation goes to.
//!
//! Pure: it reads bytes and answers values. Fetching the index, keeping the
//! last sequence and applying a version belong to the agent.

mod error;
mod index;
mod rollout;
mod signature;

pub use error::IndexError;
pub use index::{
    Channel, Index, Module, Package, PlatformVersion, Rollout, FORMAT, RESERVED_MODULE_NAMES,
};
pub use rollout::{bucket, installation_hash};
pub use signature::{verify, IndexKey};
