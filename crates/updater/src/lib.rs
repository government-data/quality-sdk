//! The update engine of an installation (decision 0001), brought from the
//! `quality-data` repository's `crates/updater` without changing what it
//! does.
//!
//! An update is a state machine ([`engine`]) whose phase is stored
//! ([`state`]) before each step that changes something, so a cut halfway goes
//! on from the stored phase and every step can run twice. When it starts
//! follows the administrator's [`policy`]. Everything that touches the world
//! (where versions come from, quality-data's API, Docker Compose, PostgreSQL,
//! the files) goes through the [`engine::Host`] trait, implemented by
//! adapters outside this crate; the tests use a fake.

pub mod engine;
pub mod policy;
pub mod state;
