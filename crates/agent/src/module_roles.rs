//! A module's machine roles as `PUT /updater/v1/modules` carries them
//! (decision 0013): each role of the manifest's `topology`, with the images
//! of the services it lists, derived from the module's `compose.yml` and
//! the `IMAGE_*` lines the agent wrote into its `.env` from the signed
//! index. A role whose images cannot all be told is left out, with a
//! warning: the core then refuses its machines rather than hand them part
//! of what they run.

use std::collections::{BTreeMap, BTreeSet};

use module_manifest::{Manifest, Role};
use serde::{Deserialize, Serialize};

/// A role as the core is told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleRole {
    pub name: String,
    pub strategy: String,
    pub services: Vec<String>,
    /// Each pinned by digest, sorted, once.
    pub images: Vec<String>,
}

#[derive(Deserialize)]
struct ComposeFile {
    #[serde(default)]
    services: BTreeMap<String, Service>,
}

#[derive(Deserialize)]
struct Service {
    #[serde(default)]
    image: Option<String>,
}

/// The roles of `manifest` (none without `topology`), from the module's
/// `compose.yml` and `.env` texts.
pub fn derive(module: &str, manifest: &Manifest, compose: &str, env: &str) -> Vec<ModuleRole> {
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
    topology
        .roles
        .iter()
        .filter_map(|role| match images(role, &services, &env) {
            Ok(images) => Some(ModuleRole {
                name: role.name.clone(),
                strategy: role.strategy.as_str().into(),
                services: role.services.clone(),
                images,
            }),
            Err(why) => {
                tracing::warn!(%module, role = %role.name, "the role is not told: {why}");
                None
            }
        })
        .collect()
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
