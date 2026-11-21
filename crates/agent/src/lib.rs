//! The installation's agent (decision 0003): it runs the update engine
//! (`updater`) with the real host, made of one adapter per concern, reading
//! its settings ([`config`]) and keeping its files at the installation's
//! [`paths`].
//!
//! Installations pull: the agent opens no port; it reads the signed index
//! and pulls what a version names.

pub mod config;
pub mod paths;
