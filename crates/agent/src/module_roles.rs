//! A module's machine roles as `PUT /updater/v1/modules` carries them
//! (decision 0013): each role of the manifest's `topology`, with the images
//! of the services it lists, derived from the module's `compose.yml` and
//! the `IMAGE_*` lines the agent wrote into its `.env` from the signed
//! index. A role whose images cannot all be told is left out, with a
//! warning: the core then refuses its machines rather than hand them part
//! of what they run. Each role also carries its files ([`crate::role_files`]):
//! `compose.yml` and what its services bind mount from the package; above
//! [`MAX_MODULE_BYTES`] for the module, none of its roles is told. Each
//! role carries `env`, the `IMAGE_*` values its `compose.yml` reads (ruling
//! 44); its declared secrets go to the core apart ([`crate::role_secrets`]).

use std::collections::{BTreeMap, BTreeSet};

use module_manifest::{Manifest, Role};

use crate::role_files::{self, Package, RoleFile, MAX_MODULE_BYTES};
use serde::{Deserialize, Serialize};

/// A role as the core is told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleRole {
    pub name: String,
    pub strategy: String,
    pub services: Vec<String>,
    /// Each pinned by digest, sorted, once.
    pub images: Vec<String>,
    /// `compose.yml` and the package files its services mount, sorted.
    pub files: Vec<RoleFile>,
    /// The `IMAGE_*` values `compose.yml` reads, as the `.env` sets them:
    /// not secret.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// The most `IMAGE_*` values a role carries.
pub const MAX_ROLE_ENV: usize = 32;

#[derive(Deserialize)]
struct ComposeFile {
    #[serde(default)]
    services: BTreeMap<String, Service>,
}

#[derive(Deserialize)]
struct Service {
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    volumes: Vec<serde_json::Value>,
}

/// The roles of `manifest` (none without `topology`), from the module's
/// `compose.yml` and `.env` texts and the package it was put from.
pub fn derive(
    module: &str,
    manifest: &Manifest,
    compose: &str,
    env: &str,
    package: &dyn Package,
) -> Vec<ModuleRole> {
    let Some(topology) = &manifest.topology else {
        return Vec::new();
    };
    let services = match serde_saphyr::from_str::<ComposeFile>(compose) {
        Ok(file) => file.services,
        Err(e) => {
            tracing::warn!(%module, "its roles are not told: compose.yml does not read: {e}");
            return Vec::new();
        }
    };
    let env = variables(env);
    let image_env = image_env(compose, &env);
    let told: Vec<ModuleRole> = topology
        .roles
        .iter()
        .filter_map(|role| {
            let made = images(role, &services, &env).and_then(|images| {
                let files = files(role, &services, compose, package)?;
                Ok(ModuleRole {
                    name: role.name.clone(),
                    strategy: role.strategy.as_str().into(),
                    services: role.services.clone(),
                    images,
                    files,
                    env: image_env.clone()?,
                })
            });
            made.map_err(|why| {
                tracing::warn!(%module, role = %role.name, "the role is not told: {why}");
            })
            .ok()
        })
        .collect();
    let distinct: BTreeMap<&str, usize> = told
        .iter()
        .flat_map(|r| r.files.iter().map(|f| (f.path.as_str(), f.size)))
        .collect();
    let bytes: usize = distinct.values().sum();
    if bytes > MAX_MODULE_BYTES {
        tracing::warn!(
            %module,
            bytes,
            "its roles are not told: their files are above {MAX_MODULE_BYTES} bytes"
        );
        return Vec::new();
    }
    told
}

/// `compose.yml` and the package files the role's services mount.
fn files(
    role: &Role,
    services: &BTreeMap<String, Service>,
    compose: &str,
    package: &dyn Package,
) -> Result<Vec<RoleFile>, String> {
    let mut paths = Vec::new();
    for name in &role.services {
        if let Some(service) = services.get(name) {
            paths.extend(role_files::mounted(&service.volumes)?);
        }
    }
    role_files::collect(compose, &paths, package)
}

/// The images of the role's services, or why one cannot be told.
fn images(
    role: &Role,
    services: &BTreeMap<String, Service>,
    env: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let mut images = BTreeSet::new();
    for name in &role.services {
        let service = services
            .get(name)
            .ok_or_else(|| format!("the service {name} is not in compose.yml"))?;
        let written = service
            .image
            .as_deref()
            .ok_or_else(|| format!("the service {name} has no image"))?;
        let image = resolve(written, env)
            .ok_or_else(|| format!("the service {name}'s image {written:?} is not set"))?;
        if !pinned(&image) {
            return Err(format!(
                "the service {name}'s image is not pinned by digest"
            ));
        }
        images.insert(image);
    }
    Ok(images.into_iter().collect())
}

/// Each `IMAGE_*` variable `compose` reads (`${IMAGE_…`) that `env` sets,
/// with its value, which must be pinned by digest; at most
/// [`MAX_ROLE_ENV`]. One the `.env` does not set is left out (a service
/// that needs it leaves its role out by [`images`]).
fn image_env(
    compose: &str,
    env: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    let mut found = BTreeMap::new();
    for (at, _) in compose.match_indices("${IMAGE_") {
        let name: String = compose[at + 2..]
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
            .collect();
        let Some(value) = env.get(&name).filter(|v| !v.is_empty()) else {
            continue;
        };
        if !pinned(value) {
            return Err(format!("{name} is not pinned by digest"));
        }
        found.insert(name, value.clone());
    }
    if found.len() > MAX_ROLE_ENV {
        return Err(format!(
            "compose.yml reads more than {MAX_ROLE_ENV} IMAGE_* values"
        ));
    }
    Ok(found)
}

/// `written` itself, or the variable it is (`${NAME}`, `${NAME:?…}`,
/// `${NAME?…}`, `${NAME:-…}`, `${NAME-…}`) as `env` sets it. A default is
/// never used: the agent writes every image.
fn resolve(written: &str, env: &BTreeMap<String, String>) -> Option<String> {
    let written = written.trim();
    let Some(inner) = written
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
    else {
        return (!written.contains('$')).then(|| written.to_string());
    };
    let name = inner.split([':', '?', '-']).next().unwrap_or_default();
    env.get(name).filter(|v| !v.is_empty()).cloned()
}

/// `<repository>@sha256:<64 hex>`.
fn pinned(image: &str) -> bool {
    image.split_once("@sha256:").is_some_and(|(repo, d)| {
        !repo.is_empty() && d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// A `.env`'s `NAME=value` lines.
fn variables(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

#[cfg(test)]
#[path = "module_roles_tests.rs"]
mod tests;
