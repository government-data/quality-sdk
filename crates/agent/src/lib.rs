//! The installation's agent (decision 0003): it runs the update engine
//! (`updater`) with the real [`host::AgentHost`], made of one adapter per concern, reading
//! its settings ([`config`]) and keeping its files at the installation's
//! [`paths`].
//!
//! Installations pull: the agent opens no port; it reads the signed index
//! and pulls what a version names.

pub mod api;
pub mod aws;
pub mod check;
pub mod config;
#[cfg(test)]
mod fake_exec;
pub mod host;
pub mod package_archive;
pub mod package_files;
pub mod paths;
