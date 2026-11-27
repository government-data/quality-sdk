//! A module's OIDC clients (decision 0009): the one people sign in
//! through, and its own machine client.
//!
//! ```yaml
//! auth: oidc                             # needs oidc.people
//! oidc:
//!   people:                              # needs route
//!     redirect_paths: [/bi/oauth-authorized/qd]
//!     logout_paths: [/bi/]
//!     alg: ES256                         # EdDSA (default) | ES256
//!     pkce: true                         # default true
//!     role_claim:
//!       name: bi_roles
//!       values: { author: bi-superset:Author }
//!   machine:
//!     capabilities: [sql-query]          # each in requires.capabilities
//!     audiences: [quality-jobs]          # the core's own, granted by the core
//! ```

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::permissions::Declared;
use crate::route::Route;
use crate::ManifestError;

/// The most redirect or logout paths of a people's client.
pub const MAX_OIDC_PATHS: usize = 8;
/// The most values of a role claim.
pub const MAX_ROLE_VALUES: usize = 16;
/// The most audiences a machine client declares.
pub const MAX_MACHINE_AUDIENCES: usize = 8;
/// Claims the core's provider sends already: a role claim cannot take them.
pub const RESERVED_CLAIMS: &[&str] = &[
    "sub",
    "iss",
    "aud",
    "exp",
    "iat",
    "auth_time",
    "nonce",
    "email",
    "name",
    "role",
    "policy",
];

/// The module's OIDC clients.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Oidc {
    /// The client people sign in through.
    #[serde(default)]
    pub people: Option<OidcPeople>,
    /// The module's own client, `client_credentials` only.
    #[serde(default)]
    pub machine: Option<OidcMachine>,
}

/// The people's client: confidential, its redirect URIs made by the core
/// from these paths and its own origin.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcPeople {
    /// Under `route.path`, 1 to 8.
    pub redirect_paths: Vec<String>,
    /// Under `route.path`, at most 8.
    #[serde(default)]
    pub logout_paths: Vec<String>,
    #[serde(default)]
    pub alg: TokenAlg,
    #[serde(default = "yes")]
    pub pkce: bool,
    #[serde(default)]
    pub role_claim: Option<RoleClaim>,
}

/// The algorithm of the tokens the core signs for the client.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum TokenAlg {
    #[default]
    EdDSA,
    ES256,
}

/// A claim whose values are those whose action the person holds.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleClaim {
    pub name: String,
    /// Value: the action that gives it.
    pub values: BTreeMap<String, String>,
}

/// The module's machine client.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcMachine {
    /// The capabilities it consumes, each one of `requires.capabilities`;
    /// none when its audiences are all it asks for.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Audiences it asks for beyond its capabilities' `token-audience`:
    /// the core's own (`quality-jobs`), which the core grants or refuses
    /// by the module's `requires` (decision 0009, ruling 30).
    #[serde(default)]
    pub audiences: Vec<String>,
}

fn yes() -> bool {
    true
}

fn invalid(why: String) -> ManifestError {
    ManifestError::Invalid(why)
}

/// `auth: oidc` needs `oidc.people`, which needs `route`; a machine client
/// consumes required capabilities only.
pub(crate) fn validate(
    route: Option<&Route>,
    oidc_auth: bool,
    oidc: Option<&Oidc>,
    required: &[&str],
    declared: &Declared<'_>,
) -> Result<(), ManifestError> {
    let people = oidc.and_then(|o| o.people.as_ref());
    if oidc_auth && people.is_none() {
        return Err(invalid("auth: oidc needs oidc.people".into()));
    }
    if let Some(p) = people {
        let Some(route) = route else {
            return Err(invalid("oidc.people needs a route".into()));
        };
        validate_people(p, route, declared)?;
    }
    if let Some(m) = oidc.and_then(|o| o.machine.as_ref()) {
        validate_machine(m, required)?;
    }
    Ok(())
}

fn validate_people(
    p: &OidcPeople,
    route: &Route,
    declared: &Declared<'_>,
) -> Result<(), ManifestError> {
    if p.redirect_paths.is_empty() {
        return Err(invalid("oidc.people.redirect_paths: none".into()));
    }
    for (what, paths) in [
        ("oidc.people.redirect_paths", &p.redirect_paths),
        ("oidc.people.logout_paths", &p.logout_paths),
    ] {
        if paths.len() > MAX_OIDC_PATHS {
            return Err(invalid(format!("{what}: at most {MAX_OIDC_PATHS}")));
        }
        for path in paths {
            if !crate::route::under(path, &route.path) {
                return Err(invalid(format!(
                    "{what}: {path:?} is not under the route {:?}",
                    route.path
                )));
            }
        }
    }
    match &p.role_claim {
        Some(claim) => validate_claim(claim, declared),
        None => Ok(()),
    }
}

fn validate_claim(claim: &RoleClaim, declared: &Declared<'_>) -> Result<(), ManifestError> {
    let name_ok = (1..=32).contains(&claim.name.len())
        && claim
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '_');
    if !name_ok {
        return Err(invalid(format!(
            "oidc.people.role_claim.name {:?}: 1 to 32 of [a-z_]",
            claim.name
        )));
    }
    if RESERVED_CLAIMS.contains(&claim.name.as_str()) {
        return Err(invalid(format!(
            "oidc.people.role_claim.name {:?}: the provider sends it already",
            claim.name
        )));
    }
    if claim.values.is_empty() || claim.values.len() > MAX_ROLE_VALUES {
        return Err(invalid(format!(
            "oidc.people.role_claim.values: 1 to {MAX_ROLE_VALUES}"
        )));
    }
    for (value, action) in &claim.values {
        let ok = (1..=32).contains(&value.len())
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
        if !ok {
            return Err(invalid(format!(
                "oidc.people.role_claim.values: {value:?}: 1 to 32 of [A-Za-z0-9_-]"
            )));
        }
        declared.check("oidc.people.role_claim.values", action)?;
    }
    Ok(())
}

fn validate_machine(m: &OidcMachine, required: &[&str]) -> Result<(), ManifestError> {
    // Audiences alone are enough (ruling 94): a client of the core's own
    // routes consumes no capability.
    if m.capabilities.is_empty() && m.audiences.is_empty() {
        return Err(invalid(
            "oidc.machine: no capabilities and no audiences".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for c in &m.capabilities {
        if !required.contains(&c.as_str()) {
            return Err(invalid(format!(
                "oidc.machine.capabilities: {c:?} is not in requires.capabilities"
            )));
        }
        if !seen.insert(c) {
            return Err(invalid(format!(
                "oidc.machine.capabilities: {c:?} appears twice"
            )));
        }
    }
    validate_audiences(&m.audiences)
}

fn validate_audiences(audiences: &[String]) -> Result<(), ManifestError> {
    if audiences.len() > MAX_MACHINE_AUDIENCES {
        return Err(invalid(format!(
            "oidc.machine.audiences: at most {MAX_MACHINE_AUDIENCES}"
        )));
    }
    let mut seen = BTreeSet::new();
    for a in audiences {
        let ok = (1..=64).contains(&a.len())
            && a.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !ok {
            return Err(invalid(format!(
                "oidc.machine.audiences: {a:?}: 1 to 64 of [A-Za-z0-9._-]"
            )));
        }
        if !seen.insert(a) {
            return Err(invalid(format!(
                "oidc.machine.audiences: {a:?} appears twice"
            )));
        }
    }
    Ok(())
}
