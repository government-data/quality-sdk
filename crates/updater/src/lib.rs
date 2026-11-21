//! The update engine of an installation (decision 0001), brought from the
//! `quality-data` repository's `crates/updater` without changing what it
//! does.
//!
//! An update's phase is stored ([`state`]) before each step that changes
//! something, so a cut halfway goes on from the stored phase. When an update
//! starts follows the administrator's [`policy`].

pub mod policy;
pub mod state;
