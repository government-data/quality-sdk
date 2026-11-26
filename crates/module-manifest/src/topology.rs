//! A module's machine roles (`topology`, decision 0013): each role is a
//! machine role a joined machine's certificate carries, the services of
//! the module's `compose.yml` it runs, and how its machines are updated.
//! What a role gets (its images and files) is derived from those services
//! by the agent, which can read `compose.yml`; this crate cannot.

use serde::Deserialize;

use crate::validate::{check_name, check_service, check_unique, invalid};
use crate::ManifestError;

/// The most roles a module declares.
pub const MAX_ROLES: usize = 16;
/// The most services a role runs.
pub const MAX_ROLE_SERVICES: usize = 32;
/// The most secrets a role declares.
pub const MAX_ROLE_SECRETS: usize = 32;
/// Roles no module may declare: the master's own certificates, which
/// never join (the core's `app` and the agent's `updater`).
pub const RESERVED_ROLES: &[&str] = &["app", "updater"];

/// `topology` in `module.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    #[serde(default)]
    pub roles: Vec<Role>,
}

/// A machine role and the services it runs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    /// As the machine's certificate names it.
    pub name: String,
    /// Services of the module's `compose.yml`.
    pub services: Vec<String>,
    pub strategy: Strategy,
    /// Names of the module's secret files (`secrets/<name>`) its machines
    /// get from the core, over mTLS only (decision 0013, ruling 44).
    #[serde(default)]
    pub secrets: Vec<String>,
}

/// How the machines of a role are updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// One machine at a time: drain, update, verify.
    One,
    /// Every machine together, in a planned short stop.
    All,
}

impl Strategy {
    /// As the manifest writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::One => "one",
            Self::All => "all",
        }
    }
}

pub(crate) fn validate(topology: Option<&Topology>) -> Result<(), ManifestError> {
    let Some(t) = topology else { return Ok(()) };
    if t.roles.len() > MAX_ROLES {
        return Err(invalid(format!(
            "topology.roles: at most {MAX_ROLES} roles"
        )));
    }
    for r in &t.roles {
        check_name("topology.roles", &r.name)?;
        if RESERVED_ROLES.contains(&r.name.as_str()) {
            return Err(invalid(format!(
                "topology.roles {:?}: the role is the master's own, which never joins",
                r.name
            )));
        }
        let what = format!("topology.roles {} services", r.name);
        if r.services.is_empty() || r.services.len() > MAX_ROLE_SERVICES {
            return Err(invalid(format!(
                "{what}: 1 to {MAX_ROLE_SERVICES} services"
            )));
        }
        for s in &r.services {
            check_service(&what, s)?;
        }
        check_unique(&what, r.services.iter().map(String::as_str))?;
        check_secrets(r)?;
    }
    check_unique("topology.roles", t.roles.iter().map(|r| r.name.as_str()))
}

/// At most [`MAX_ROLE_SECRETS`], each `[A-Za-z0-9._-]`, 1 to 64, once.
fn check_secrets(r: &Role) -> Result<(), ManifestError> {
    let what = format!("topology.roles {} secrets", r.name);
    if r.secrets.len() > MAX_ROLE_SECRETS {
        return Err(invalid(format!("{what}: at most {MAX_ROLE_SECRETS}")));
    }
    for s in &r.secrets {
        let ok = (1..=64).contains(&s.len())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            && s != "."
            && s != "..";
        if !ok {
            return Err(invalid(format!(
                "{what}: {s:?}: 1 to 64 of [A-Za-z0-9._-], a file's name"
            )));
        }
    }
    check_unique(&what, r.secrets.iter().map(String::as_str))
}
