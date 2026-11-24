//! `qd module add|remove` (decision 0002 step 9 and its amendment of
//! 2026-11-23, point 6): a module of the installed platform version added
//! to the installation, or removed from it, with the checks of
//! `module-manifest`.
//!
//! - **add** takes the module at the installed platform version from the
//!   signed index, refuses it when a capability it requires is not met (an
//!   optional one warns), puts its package and images, brings it up, writes
//!   its route, waits for its health and tells quality-data.
//! - **remove** refuses while an installed module requires a capability
//!   only this one provides (naming both); otherwise it brings it down
//!   (its volumes kept), takes it out of what is installed, removes its
//!   route and tells quality-data. Its package, `.env` and data stay.
//!
//! Both run under the agent's lock (the service stopped) and only while no
//! update runs; a version prepared before is set back to idle, to be
//! prepared again with the modules installed now (decision 0003).

use std::path::{Path, PathBuf};

use module_manifest::{lost_on_removal, unmet, Auth, Manifest};
use updater::engine::{Installed, Offer, OfferedPackage};
use updater::state::Modules;

pub mod adapters;

/// The module the core is: never removed.
const CORE: &str = "quality-data";

/// What `add` and `remove` act on.
pub trait ModuleHost {
    /// Refuse while an update runs; a prepared version goes back to idle.
    fn no_update_running(&self) -> Result<(), String>;
    /// The installed platform version, its modules and their start order.
    fn installed(&self) -> Result<(Installed, Vec<String>), String>;
    /// An installed module's manifest.
    fn manifest(&self, module: &str) -> Result<Option<Manifest>, String>;
    /// The signed index's platform version `version`.
    fn offer(&self, version: &str) -> Result<Offer, String>;
    fn download(&self, offer: &Offer, package: &OfferedPackage) -> Result<PathBuf, String>;
    /// The manifest inside a downloaded package.
    fn package_manifest(&self, package: &Path) -> Result<Option<Manifest>, String>;
    fn pull_images(&self, offer: &Offer) -> Result<(), String>;
    fn put_package(&self, module: &str, package: &Path) -> Result<(), String>;
    /// Record `modules` as installed at `version`, starting in `order`.
    fn put_modules(&self, version: &str, modules: &Modules, order: &[String])
        -> Result<(), String>;
    fn up(&self, module: &str) -> Result<(), String>;
    fn down(&self, module: &str) -> Result<(), String>;
    fn healthy(&self, module: &str) -> Result<(), String>;
    /// Every installed module's route written, the others' removed.
    fn routes(&self) -> Result<(), String>;
    /// Tell quality-data the installed modules.
    fn tell_core(&self) -> Result<(), String>;
}

/// What a command did: its warnings, for the person.
pub type Warnings = Vec<String>;

/// Add `module` at the installed platform version.
pub fn add(host: &dyn ModuleHost, module: &str) -> Result<Warnings, String> {
    host.no_update_running()?;
    let (installed, order) = host.installed()?;
    if installed.modules.contains_key(module) {
        return Err(format!("{module} is already installed"));
    }
    let offer = only(host.offer(&installed.version)?, module).ok_or_else(|| {
        format!(
            "the platform version {} has no module {module}",
            installed.version
        )
    })?;
    let package = host.download(&offer, &offer.packages[0])?;
    let manifest = host
        .package_manifest(&package)?
        .ok_or_else(|| format!("{module}'s package has no module.yaml"))?;
    if manifest.name != module {
        return Err(format!(
            "{module}'s package is the module {:?}",
            manifest.name
        ));
    }
    if manifest.auth == Some(Auth::Oidc) || manifest.oidc.is_some() {
        return Err(format!(
            "{module} signs in with OIDC, which modules cannot do yet"
        ));
    }
    if manifest.permissions.is_some() {
        return Err(format!(
            "{module} declares permissions, which modules cannot have yet"
        ));
    }
    if !manifest.secrets.generate.is_empty() || manifest.database_role().is_some() {
        return Err(format!(
            "{module} asks for secret files or a database, which modules cannot get yet"
        ));
    }
    let manifests = manifests(host, installed.modules.keys())?;
    let mut warnings = Vec::new();
    let mut missing = Vec::new();
    for u in unmet(&manifest, &manifests) {
        let what = format!(
            "{module} requires {} {} ({:?})",
            u.capability, u.version, u.why
        );
        if u.optional {
            warnings.push(format!("{what}: optional, added without it"));
        } else {
            missing.push(what);
        }
    }
    if !missing.is_empty() {
        return Err(format!("not added: {}", missing.join("; ")));
    }
    host.pull_images(&offer)?;
    host.put_package(module, &package)?;
    let mut modules = installed.modules.clone();
    modules.extend(offer.modules.clone());
    let mut order = order;
    order.push(module.to_string());
    host.put_modules(&installed.version, &modules, &order)?;
    host.up(module)?;
    host.routes()?;
    host.healthy(module)?;
    tell(host, &mut warnings);
    Ok(warnings)
}

/// Remove `module`, keeping its data.
pub fn remove(host: &dyn ModuleHost, module: &str) -> Result<Warnings, String> {
    host.no_update_running()?;
    let (installed, order) = host.installed()?;
    if !installed.modules.contains_key(module) {
        return Err(format!("{module} is not installed"));
    }
    if module == CORE {
        return Err(format!("{CORE} is the core: it cannot be removed"));
    }
    let manifests = manifests(host, installed.modules.keys())?;
    let mut warnings = Vec::new();
    let mut refused = Vec::new();
    for l in lost_on_removal(module, &manifests) {
        let what = format!(
            "{} requires {} {} from {module}",
            l.module, l.capability, l.version
        );
        if l.optional {
            warnings.push(format!("{what}: optional, it goes on without it"));
        } else {
            refused.push(what);
        }
    }
    if !refused.is_empty() {
        return Err(format!("not removed: {}", refused.join("; ")));
    }
    host.down(module)?;
    let mut modules = installed.modules.clone();
    modules.remove(module);
    let order: Vec<String> = order.into_iter().filter(|m| m != module).collect();
    host.put_modules(&installed.version, &modules, &order)?;
    host.routes()?;
    tell(host, &mut warnings);
    Ok(warnings)
}

/// `offer` with `module` alone; none when the version does not have it.
fn only(mut offer: Offer, module: &str) -> Option<Offer> {
    let entry = offer.modules.remove(module)?;
    let package = offer.packages.into_iter().find(|p| p.module == module)?;
    offer.modules = Modules::from([(module.to_string(), entry)]);
    offer.packages = vec![package];
    Some(offer)
}

fn manifests<'a>(
    host: &dyn ModuleHost,
    modules: impl Iterator<Item = &'a String>,
) -> Result<Vec<Manifest>, String> {
    let mut all = Vec::new();
    for m in modules {
        all.extend(host.manifest(m)?);
    }
    Ok(all)
}

/// Telling quality-data may fail (the service tells it when it starts).
fn tell(host: &dyn ModuleHost, warnings: &mut Warnings) {
    if let Err(err) = host.tell_core() {
        warnings.push(format!(
            "quality-data was not told the modules ({err}); the service tells it when it starts"
        ));
    }
}

#[cfg(test)]
mod tests;
