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
//! What a module gets (decision 0009, [`provision`]): at `add`, its secret
//! files, its role and databases and its OIDC clients, each step written
//! to its progress file before it runs, so a later `add` resumes; at
//! `remove`, its clients disabled and its role without `LOGIN`, every
//! secret and database kept for an `add` afterwards. Both are audited.
//!
//! Both run under the agent's lock (the service stopped) and only while no
//! update runs; a version prepared before is set back to idle, to be
//! prepared again with the modules installed now (decision 0003).

use std::path::{Path, PathBuf};

use module_manifest::{lost_on_removal, unmet, Manifest};
use updater::engine::{Installed, Offer, OfferedPackage};
use updater::state::Modules;

pub mod adapters;
pub mod provision;
pub mod provisioner;
pub mod rotate;

pub use provision::{Progress, Provision};

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
    /// What a module gets: secrets, database, clients (decision 0009).
    fn provision(&self) -> &dyn Provision;
}

/// What a command did: its warnings, for the person.
pub type Warnings = Vec<String>;

/// Add `module` at the installed platform version; with a progress file
/// of an `add` that stopped, resume it (every step runs again, changing
/// nothing that is done).
pub fn add(host: &dyn ModuleHost, module: &str) -> Result<Warnings, String> {
    host.no_update_running()?;
    let p = host.provision();
    let resuming = match p.progress(module)? {
        Some(pr) if pr.operation == "purging" => {
            return Err(format!(
                "a purge of {module} is under way: run qd module purge {module} again"
            ))
        }
        Some(pr) => pr.operation == "adding",
        None => false,
    };
    let (installed, order) = host.installed()?;
    if installed.modules.contains_key(module) && !resuming {
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
    let others = installed.modules.keys().filter(|m| *m != module);
    let manifests = manifests(host, others)?;
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
    if manifest.oidc.is_some() || manifest.permissions.is_some() {
        provision::check_features(&manifest, &p.features()?)?;
    }
    let audiences = provision::audiences(&manifest, &manifests)?;
    let version = manifest.version.to_string();
    let step = std::cell::RefCell::new(String::from("prepared"));
    let mut save = |s: &str| {
        *step.borrow_mut() = s.to_string();
        p.save_progress(
            module,
            &Progress {
                operation: "adding".into(),
                version: version.clone(),
                step: s.into(),
            },
        )
    };
    let added = (|| {
        host.pull_images(&offer)?;
        host.put_package(module, &package)?;
        save("prepared")?;
        if provision::provides_anything(&manifest) {
            provision::provision(p, &manifest, &audiences, &mut save)?;
        }
        save("started")?;
        let mut modules = installed.modules.clone();
        modules.extend(offer.modules.clone());
        let mut order = order;
        order.retain(|m| m != module);
        order.push(module.to_string());
        host.put_modules(&installed.version, &modules, &order)?;
        host.up(module)?;
        host.routes()?;
        host.healthy(module)
    })();
    if let Err(err) = added {
        let at = step.borrow().clone();
        p.audit(&provision::event(
            "ModuleAdded",
            module,
            &version,
            Some((&at, &err)),
        ));
        return Err(format!(
            "{err} ({module} stopped at {at}: qd module add {module} resumes it)"
        ));
    }
    tell(host, &mut warnings);
    p.audit(&provision::event("ModuleAdded", module, &version, None));
    p.clear_progress(module)?;
    Ok(warnings)
}

/// Remove `module`, keeping its data, its secrets, its role and its
/// databases; its clients disabled and its role without `LOGIN`. An `add`
/// that stopped is undone as far as it went.
pub fn remove(host: &dyn ModuleHost, module: &str) -> Result<Warnings, String> {
    host.no_update_running()?;
    let p = host.provision();
    let progress = p.progress(module)?;
    let (installed, order) = host.installed()?;
    let is_installed = installed.modules.contains_key(module);
    let adding = progress.as_ref().is_some_and(|pr| pr.operation == "adding");
    if !is_installed && !adding {
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
    let manifest = host.manifest(module)?;
    host.down(module)?;
    if is_installed {
        let mut modules = installed.modules.clone();
        modules.remove(module);
        let order: Vec<String> = order.into_iter().filter(|m| m != module).collect();
        host.put_modules(&installed.version, &modules, &order)?;
    }
    host.routes()?;
    let mut left = false;
    if let Some(m) = &manifest {
        left = provision::deprovision(p, m, &mut warnings)?;
    }
    tell(host, &mut warnings);
    let version = manifest
        .as_ref()
        .map(|m| m.version.to_string())
        .unwrap_or_default();
    p.audit(&provision::event("ModuleRemoved", module, &version, None));
    if left {
        p.save_progress(
            module,
            &Progress {
                operation: "removing".into(),
                version,
                step: "clients".into(),
            },
        )?;
    } else {
        p.clear_progress(module)?;
    }
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
mod fake_provision;
#[cfg(test)]
mod provision_tests;
#[cfg(test)]
mod tests;
