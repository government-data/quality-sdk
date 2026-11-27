//! The manifest's model, as `module.yaml` writes it.

use std::collections::BTreeMap;

use serde::de::IgnoredAny;
use serde::Deserialize;

pub use crate::data::{Backup, Data, Database, Key, Volume};
use crate::events::{Consumes, ProvidedEvent};
use crate::oidc::Oidc;
use crate::permissions::Permissions;
use crate::route::{Auth, Menu, Route};
use crate::secrets::Secrets;
use crate::topology::Topology;
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
    /// Where the core asks the module whether another (or itself) may be
    /// removed (decision 0015; none: it never refuses).
    #[serde(default)]
    pub removal: Option<Removal>,
    /// Where it is served to people (none: it is not).
    #[serde(default)]
    pub route: Option<Route>,
    /// How a person gets through its route (needs `route`).
    #[serde(default)]
    pub auth: Option<Auth>,
    /// Its entry in the core's apps menu (needs `route`).
    #[serde(default)]
    pub menu: Option<Menu>,
    /// Its OIDC clients (decision 0009).
    #[serde(default)]
    pub oidc: Option<Oidc>,
    /// The secret files the agent generates for it (decision 0009).
    #[serde(default)]
    pub secrets: Secrets,
    /// Its own actions and managed policies (decision 0009).
    #[serde(default)]
    pub permissions: Option<Permissions>,
    /// Its machine roles (decision 0013; none: every service runs on the
    /// master).
    #[serde(default)]
    pub topology: Option<Topology>,
    /// The events it reads (decision 0017).
    #[serde(default)]
    pub consumes: Consumes,
    // Sections later steps of decision 0002 define: accepted, not read yet.
    #[serde(default)]
    images: Option<IgnoredAny>,
    #[serde(default)]
    config: Option<IgnoredAny>,
    #[serde(default)]
    requirements: Option<IgnoredAny>,
}

/// What a module provides.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provides {
    #[serde(default)]
    pub capabilities: Vec<ProvidedCapability>,
    /// The topics it publishes (decision 0017).
    #[serde(default)]
    pub events: Vec<ProvidedEvent>,
    // Defined by later steps of decision 0002.
    #[serde(default)]
    apis: Option<IgnoredAny>,
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

/// An HTTP endpoint the module serves on its route of the `internal`
/// entry point (Traefik, mTLS: the core calls it with its machine
/// certificate). The core calls it with `?horizon_secs=N` and it answers
/// `{"running": <n>, "scheduled_soon": <n>}`; the core sums the answers of
/// every installed module, whose paths the agent tells it (decision 0002).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drain {
    /// Absolute (`/` first), without a query: the core adds it.
    pub path: String,
}

/// The module's removal check (decision 0015), on its route of the
/// `internal` entry point, as [`Drain`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Removal {
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
    /// Services that run once and end, such as a migration before the
    /// web service starts (decision 0002, amended 2026-11-25): a container
    /// of one that exited with code 0 is healthy; one that exited with
    /// another code, or is still running, is judged as any other.
    #[serde(default)]
    pub oneshot: Vec<String>,
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

impl Manifest {
    /// Parse and validate a manifest's text.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let manifest: Self =
            serde_saphyr::from_str(text).map_err(|e| ManifestError::Malformed(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }
}

impl Manifest {
    /// The databases the agent creates for the module.
    pub fn created_databases(&self) -> impl Iterator<Item = &Database> {
        self.data.databases.iter().filter(|d| d.create)
    }

    /// The agent version that first reads the keys this manifest uses, the
    /// highest of them: [`crate::REPLICA_BACKUP_AGENT`] for a volume that
    /// writes `backup`, `dump` or `replica` (decision 0002; left out, it
    /// is `dump` and needs nothing); [`crate::MODULE_ADD_AGENT`] for
    /// `oidc`, `secrets`, `data.databases[].create`, `menu.requires` or
    /// `permissions` (decision 0009); [`crate::SECRET_KIND_AGENT`] for a
    /// written `secrets.generate[].kind` (decision 0012);
    /// [`crate::ROLES_AGENT`] for `topology` (decision 0013);
    /// [`crate::ONESHOT_AGENT`] for `health.oneshot` (decision 0002);
    /// [`crate::MACHINE_AUDIENCES_AGENT`] for `oidc.machine.audiences`
    /// (decision 0009); [`crate::ROLE_SECRETS_AGENT`] for
    /// `topology.roles[].secrets` (decision 0013);
    /// [`crate::ROLE_SECRETS_FROM_AGENT`] for a secret's `from` (ruling 96);
    /// [`crate::REMOVAL_AGENT`]
    /// for `removal` (decision 0015); none for a manifest that uses none of
    /// them.
    pub fn agent_needed(&self) -> Option<semver::Version> {
        let module_add = self.oidc.is_some()
            || self.permissions.is_some()
            || !self.secrets.generate.is_empty()
            || self.secrets.uid != 0
            || self.created_databases().next().is_some()
            || self.menu.as_ref().is_some_and(|m| m.requires.is_some());
        let backup = self.data.volumes.iter().any(Volume::backup_written);
        let kind = self.secrets.generate.iter().any(|g| g.kind.is_some());
        let audiences = self.machine_audiences().next().is_some();
        let roles = || self.topology.iter().flat_map(|t| &t.roles);
        let role_secrets = roles().any(|r| !r.secrets.is_empty());
        let shared = roles().flat_map(|r| &r.secrets).any(|s| s.from().is_some());
        let version = |v: &str| semver::Version::parse(v).expect("a version");
        let needed = [
            module_add.then(|| version(crate::MODULE_ADD_AGENT)),
            backup.then(|| version(crate::REPLICA_BACKUP_AGENT)),
            kind.then(|| version(crate::SECRET_KIND_AGENT)),
            self.topology.is_some().then(|| version(crate::ROLES_AGENT)),
            (!self.health.oneshot.is_empty()).then(|| version(crate::ONESHOT_AGENT)),
            self.declares_events().then(|| version(crate::EVENTS_AGENT)),
            audiences.then(|| version(crate::MACHINE_AUDIENCES_AGENT)),
            role_secrets.then(|| version(crate::ROLE_SECRETS_AGENT)),
            shared.then(|| version(crate::ROLE_SECRETS_FROM_AGENT)),
            self.removal
                .is_some()
                .then(|| version(crate::REMOVAL_AGENT)),
        ];
        needed.into_iter().flatten().max()
    }

    /// The audiences its machine client declares (`oidc.machine.audiences`).
    pub fn machine_audiences(&self) -> impl Iterator<Item = &String> {
        self.oidc
            .iter()
            .filter_map(|o| o.machine.as_ref())
            .flat_map(|m| &m.audiences)
    }

    /// Whether it provides or consumes any event (decision 0017).
    pub fn declares_events(&self) -> bool {
        !self.provides.events.is_empty() || !self.consumes.events.is_empty()
    }

    /// The module's PostgreSQL role, when it has a database the agent
    /// creates: its name with `-` as `_`.
    pub fn database_role(&self) -> Option<String> {
        self.created_databases()
            .next()
            .map(|_| crate::secrets::database_prefix(&self.name))
    }
}

impl ProviderHold {
    /// `ttl` in seconds (validated on parse).
    pub fn ttl_secs(&self) -> u64 {
        crate::validate::duration_secs(&self.ttl).unwrap_or(0)
    }
}
