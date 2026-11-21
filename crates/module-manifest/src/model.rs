//! The manifest's model, as `module.yaml` writes it.

use std::collections::BTreeMap;

use serde::de::IgnoredAny;
use serde::Deserialize;

use crate::ManifestError;

/// How long a health check may take when the manifest does not say.
pub const CHECK_TIMEOUT_SECS: u64 = 30;
/// The longest a health check may take.
pub const MAX_CHECK_TIMEOUT_SECS: u64 = 300;

/// A module's `module.yaml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The module's name: its folder, its Compose project.
    pub name: String,
    pub version: semver::Version,
    #[serde(default)]
    pub provides: Provides,
    #[serde(default)]
    pub requires: Requires,
    /// What is checked beyond its containers' Compose healthchecks.
    #[serde(default)]
    pub health: Health,
    /// What its data is: what a backup covers.
    #[serde(default)]
    pub data: Data,
    /// Where the core asks what work runs before an update (none: the
    /// module has no work to drain).
    #[serde(default)]
    pub drain: Option<Drain>,
    // Sections later steps of decision 0002 define: accepted, not read yet.
    #[serde(default)]
    images: Option<IgnoredAny>,
    #[serde(default)]
    route: Option<IgnoredAny>,
    #[serde(default)]
    auth: Option<IgnoredAny>,
    #[serde(default)]
    menu: Option<IgnoredAny>,
    #[serde(default)]
    consumes: Option<IgnoredAny>,
    #[serde(default)]
    permissions: Option<IgnoredAny>,
    #[serde(default)]
    config: Option<IgnoredAny>,
    #[serde(default)]
    topology: Option<IgnoredAny>,
    #[serde(default)]
    requirements: Option<IgnoredAny>,
}

/// What a module provides.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provides {
    #[serde(default)]
    pub capabilities: Vec<ProvidedCapability>,
    // Defined by later steps of decision 0002.
    #[serde(default)]
    apis: Option<IgnoredAny>,
    #[serde(default)]
    events: Option<IgnoredAny>,
}

/// A capability a module provides.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidedCapability {
    pub name: String,
    pub version: u32,
    /// Every protocol it promises.
    pub protocols: Vec<String>,
    /// What it supports or not, declared, never assumed.
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub hold: Option<ProviderHold>,
}

/// How a provider's agent may hold the capability's consumers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderHold {
    /// The longest a hold lasts: `<n>s`, `<n>m` or `<n>h`.
    pub ttl: String,
    /// The machine roles whose certificate may ask for a hold.
    pub roles: Vec<String>,
}

/// What a module requires.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requires {
    #[serde(default)]
    pub capabilities: Vec<RequiredCapability>,
}

/// A capability a module requires: by name, version and the protocols it
/// uses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredCapability {
    pub name: String,
    pub version: u32,
    pub protocols: Vec<String>,
    /// Missing, it warns instead of refusing.
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub hold: Option<ConsumerHold>,
}

/// Where the core asks a consumer to hold the jobs that use the capability.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumerHold {
    pub path: String,
}

/// An HTTP endpoint the module serves on its internal service inside the
/// Compose network. The core calls it with `?horizon_secs=N` and it
/// answers `{"running": <n>, "scheduled_soon": <n>}`; the core sums the
/// answers of every installed module (decision 0002).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drain {
    /// Absolute (`/` first), without a query: the core adds it.
    pub path: String,
}

/// A module's health beyond its containers' Compose healthchecks, which are
/// always required.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    /// Services that must each have a running container.
    #[serde(default)]
    pub services: Vec<String>,
    /// Commands that must succeed.
    #[serde(default)]
    pub checks: Vec<Check>,
}

/// A command run in a running container of a service: exit 0 is healthy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub service: String,
    /// The program and its arguments, run without a shell.
    pub command: Vec<String>,
    #[serde(default = "check_timeout")]
    pub timeout_secs: u64,
}

fn check_timeout() -> u64 {
    CHECK_TIMEOUT_SECS
}

/// A module's data. A critical piece is backed up before every update; one
/// that is not (a cache) is kept on remove but not backed up.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    #[serde(default)]
    pub databases: Vec<Database>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub keys: Vec<Key>,
}

/// A database of the installation's PostgreSQL.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    pub name: String,
    #[serde(default = "critical")]
    pub critical: bool,
}

/// A Compose volume, read through a service whose container mounts it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Volume {
    /// Names it in the backup.
    pub name: String,
    pub service: String,
    /// Where it is mounted in that service (absolute).
    pub path: String,
    #[serde(default = "critical")]
    pub critical: bool,
}

/// A file under the module's directory (`/opt/quality-data/<module>/`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    /// Relative to the module's directory.
    pub path: String,
    #[serde(default = "critical")]
    pub critical: bool,
}

fn critical() -> bool {
    true
}

impl Manifest {
    /// Parse and validate a manifest's text.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let manifest: Self =
            serde_saphyr::from_str(text).map_err(|e| ManifestError::Malformed(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }
}

impl ProviderHold {
    /// `ttl` in seconds (validated on parse).
    pub fn ttl_secs(&self) -> u64 {
        crate::validate::duration_secs(&self.ttl).unwrap_or(0)
    }
}

impl Data {
    /// The critical pieces: what a backup must cover.
    pub fn critical(&self) -> Data {
        Data {
            databases: self
                .databases
                .iter()
                .filter(|d| d.critical)
                .cloned()
                .collect(),
            volumes: self
                .volumes
                .iter()
                .filter(|v| v.critical)
                .cloned()
                .collect(),
            keys: self.keys.iter().filter(|k| k.critical).cloned().collect(),
        }
    }
}
