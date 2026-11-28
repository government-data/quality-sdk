//! A role's declared secrets handed to the core (decision 0013, ruling 44):
//! `PUT /updater/v1/modules/{module}/role-secrets` with the values of each
//! told role's `secrets`, read from the module's `secrets/`, after every
//! `PUT /updater/v1/modules` (the core keeps them in memory only). Sent
//! only to a core that lists the feature `role-secrets`.
//!
//! A value travels in that request body only, over mTLS: never logged, in
//! a `Debug` print ([`Secret`] prints `[secret]`), a report or a role's
//! files. A secret that cannot be read, or that the core would refuse,
//! leaves its role without secrets, with a warning naming module, role and
//! secret: the role's machines then lack it, which is visible, rather than
//! the whole call refused. A secret declared `from` a required capability
//! (ruling 96) is read from the `secrets/` of the installed module that
//! provides it.

use std::collections::BTreeMap;

use module_manifest::{Manifest, RoleSecret};
use serde::Serialize;

use crate::module_roles::ModuleRole;
use crate::module_secrets::Secret;

/// The longest value the core takes.
pub const MAX_VALUE: usize = 64 * 1024;

/// The body of `PUT …/role-secrets`: each role's secrets by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RoleSecrets {
    pub roles: BTreeMap<String, BTreeMap<String, Secret>>,
}

/// The secrets of `manifest`'s roles among `told`, each read by `read`
/// (a module's secret by the module and its file's name: `module`'s own,
/// or a `from` capability's provider among `installed`); none when no
/// role of the manifest declares any, so nothing is sent.
pub fn of(
    module: &str,
    manifest: &Manifest,
    told: &[ModuleRole],
    installed: &[Manifest],
    read: &dyn Fn(&str, &str) -> Result<Secret, String>,
) -> Option<RoleSecrets> {
    let roles = &manifest.topology.as_ref()?.roles;
    if roles.iter().all(|r| r.secrets.is_empty()) {
        return None;
    }
    let mut body = RoleSecrets::default();
    for role in roles.iter().filter(|r| !r.secrets.is_empty()) {
        if !told.iter().any(|t| t.name == role.name) {
            continue;
        }
        let values: Result<BTreeMap<String, Secret>, (String, String)> = role
            .secrets
            .iter()
            .map(|secret| {
                let name = secret.name().to_string();
                let owner = owner(module, secret, installed).map_err(|e| (name.clone(), e))?;
                let value = read(&owner, &name).map_err(|e| (name.clone(), e))?;
                let v = value.expose();
                if v.len() > MAX_VALUE || v.contains('\0') {
                    let why = format!("above {MAX_VALUE} bytes or with a NUL");
                    return Err((name.clone(), why));
                }
                Ok((name.clone(), value))
            })
            .collect();
        match values {
            Ok(values) => {
                body.roles.insert(role.name.clone(), values);
            }
            Err((secret, why)) => tracing::warn!(
                %module,
                role = %role.name,
                %secret,
                "the role's secrets are not handed to quality-data: {why}"
            ),
        }
    }
    Some(body)
}

/// The module whose `secrets/` holds `secret`: `module`, or the installed
/// provider of its `from` capability.
fn owner(module: &str, secret: &RoleSecret, installed: &[Manifest]) -> Result<String, String> {
    let Some(capability) = secret.from() else {
        return Ok(module.to_string());
    };
    installed
        .iter()
        .find(|m| m.provides.capabilities.iter().any(|c| c.name == capability))
        .map(|m| m.name.clone())
        .ok_or_else(|| format!("no installed module provides {capability}"))
}

#[cfg(test)]
#[path = "role_secrets_tests.rs"]
mod tests;
