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
pub mod command;
pub mod compose;
pub mod config;
pub mod enrol;
#[cfg(test)]
mod fake_exec;
pub mod host;
pub mod install;
pub mod installed_module;
pub mod manifests;
pub mod module_data;
pub mod module_database;
pub mod module_secrets;
pub mod modules;
pub mod package_archive;
pub mod package_files;
pub mod paths;
pub mod postgres;
pub mod registry;
pub mod replicas;
pub mod routes;
pub mod service;
pub mod update;
