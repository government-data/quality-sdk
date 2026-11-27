//! The real [`Host`] of the update engine, composed of one adapter per
//! concern (decision 0003), each behind its own trait so it is tested on its
//! own and the composition is tested with fakes:
//!
//! | Adapter | `Host` methods |
//! | --- | --- |
//! | [`Cloud`] (AWS) | `check_cloud`, `report_to_cloud`, `report_replica_refused`, `download_package`, `pull_images` |
//! | [`Compose`] (Docker Compose) | `put_version`, `start`, `installed`, the module health of `healthy` |
//! | [`Database`] (PostgreSQL) | `back_up`, `restore_database` |
//! | [`Packages`] (package files) | `free_space`, `save_package`, `manifest`, `put_package`, `install_commands` |
//! | [`Api`] (quality-data's `updater/v1`) | telling `save`, `orders`, `maintenance`, `drain`, the version of `healthy`, `replicas` (with [`Compose`]'s manifests) |
//! | [`crate::modules::Provision`] (secrets, PostgreSQL, `updater/v1`) | `provision` (decision 0009) |
//!
//! The host also tells quality-data which modules are installed
//! ([`AgentHost::tell_modules`]) after every version it starts and when the
//! agent starts, so the core knows them (decision 0003).
//!
//! `now`, `wait` and writing the state file are the agent's own.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};

use module_manifest::Manifest;
use updater::engine::{CloudError, Drain, Host, Installed, Offer, OfferedPackage, Replica};
use updater::policy::Orders;
use updater::state::{Modules, Outcome, State};

pub use crate::installed_module::{Features, InLanguages, InstalledModule, ModuleMenu};

pub use crate::host_ports::{Api, Clock, Cloud, Compose, Database, Packages, SystemClock};

/// Every adapter the host is made of.
pub struct Adapters {
    pub cloud: Box<dyn Cloud>,
    pub compose: Box<dyn Compose>,
    pub database: Box<dyn Database>,
    pub packages: Box<dyn Packages>,
    pub api: Box<dyn Api>,
    /// What a module's package asks for (decision 0009).
    pub provision: Box<dyn crate::modules::Provision>,
    pub clock: Box<dyn Clock>,
}

impl std::fmt::Debug for Adapters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Adapters").finish_non_exhaustive()
    }
}

/// The engine's host: the adapters, and the state file it writes.
#[derive(Debug)]
pub struct AgentHost {
    adapters: Adapters,
    state_file: PathBuf,
    /// The modules whose packages were put since the last version was put,
    /// in that order: the engine puts them a module after those it
    /// requires.
    put_order: RefCell<Vec<String>>,
    /// quality-data has not been told the installed modules since the agent
    /// started or a version was put.
    modules_untold: Cell<bool>,
    /// quality-data was told the installed modules since
    /// [`AgentHost::take_told`] last asked.
    told_anew: Cell<bool>,
}

impl AgentHost {
    pub fn new(adapters: Adapters, state_file: PathBuf) -> Self {
        Self {
            adapters,
            state_file,
            put_order: RefCell::default(),
            modules_untold: Cell::new(true),
            told_anew: Cell::new(false),
        }
    }

    /// Tell quality-data every installed module (`PUT /updater/v1/modules`,
    /// the whole list, so telling it twice changes nothing). A failure is
    /// logged and told again by [`AgentHost::tell_modules_if_untold`].
    pub fn tell_modules(&self) {
        match self.put_modules() {
            Ok(()) => {
                self.modules_untold.set(false);
                self.told_anew.set(true);
            }
            Err(err) => {
                self.modules_untold.set(true);
                tracing::warn!("quality-data was not told the installed modules: {err}");
            }
        }
    }

    /// `PUT /updater/v1/modules` with every installed module, saying why
    /// when it fails.
    pub fn put_modules(&self) -> Result<(), String> {
        let compose = &self.adapters.compose;
        let features = self.adapters.api.features().unwrap_or_else(|err| {
            tracing::debug!("quality-data's features are not known ({err}): none");
            Features::default()
        });
        let i = compose.installed()?;
        let mut manifests = Vec::new();
        let modules = i
            .modules
            .iter()
            .map(|(name, m)| {
                let manifest = compose.manifest(name)?;
                let mut told = InstalledModule::new(name, &m.version, manifest.as_ref(), &features);
                if let Some(manifest) = manifest {
                    told.roles = compose.roles(name, &manifest);
                    told.events = compose.events(name, &manifest);
                    manifests.push(manifest);
                }
                Ok(told)
            })
            .collect::<Result<Vec<_>, String>>()?;
        self.adapters.api.put_modules(&modules)?;
        if features.has(Features::ROLE_SECRETS) {
            self.hand_role_secrets(&modules, &manifests)?;
        }
        Ok(())
    }

    /// Each module's roles' declared secrets to the core, after the list
    /// that names its roles (decision 0013, ruling 44).
    fn hand_role_secrets(
        &self,
        modules: &[InstalledModule],
        manifests: &[Manifest],
    ) -> Result<(), String> {
        let provision = &self.adapters.provision;
        for manifest in manifests {
            let Some(told) = modules.iter().find(|m| m.name == manifest.name) else {
                continue;
            };
            let read = |owner: &str, name: &str| provision.secret(owner, name);
            if let Some(body) =
                crate::role_secrets::of(&manifest.name, manifest, &told.roles, manifests, &read)
            {
                self.adapters.api.put_role_secrets(&manifest.name, &body)?;
            }
        }
        Ok(())
    }

    /// [`AgentHost::tell_modules`] when quality-data has not been told them
    /// since the agent started or the last version was put (or telling it
    /// failed): the service does it before each round.
    pub fn tell_modules_if_untold(&self) {
        if self.modules_untold.get() {
            self.tell_modules();
        }
    }

    /// Whether quality-data was told the installed modules since the last
    /// call (at start, or after a version was put: its API may have
    /// restarted and forgotten what it keeps in memory).
    pub fn take_told(&self) -> bool {
        self.told_anew.replace(false)
    }
}

impl Host for AgentHost {
    fn now(&self) -> DateTime<Utc> {
        self.adapters.clock.now()
    }
    fn wait(&self, d: Duration) {
        self.adapters.clock.wait(d);
    }
    /// The state file first, then the API; the API may be updating, so its
    /// failure is ignored.
    fn save(&self, s: &State) -> Result<(), String> {
        s.save(&self.state_file)?;
        if let Err(err) = self.adapters.api.put_state(s) {
            tracing::debug!("the API was not told the state: {err}");
        }
        Ok(())
    }
    fn check_cloud(&self, installed_version: &str) -> Result<Offer, CloudError> {
        self.adapters.cloud.check(installed_version)
    }
    fn report_to_cloud(&self, outcome: &Outcome) {
        self.adapters.cloud.report(outcome);
    }
    fn free_space(&self) -> Result<(), String> {
        self.adapters.packages.free_space()
    }
    fn download_package(&self, offer: &Offer, p: &OfferedPackage) -> Result<PathBuf, String> {
        self.adapters.cloud.download_package(offer, p)
    }
    fn pull_images(&self, offer: &Offer) -> Result<(), String> {
        self.adapters.cloud.pull_images(offer)
    }
    fn orders(&self) -> Orders {
        self.adapters.api.orders()
    }
    fn maintenance(&self, reason: Option<&str>, lasts: Duration) -> Result<(), String> {
        self.adapters.api.maintenance(reason, lasts)
    }
    fn drain(&self, horizon: Duration) -> Result<Drain, String> {
        self.adapters.api.drain(horizon)
    }
    fn installed(&self) -> Result<Installed, String> {
        self.adapters.compose.installed()
    }
    fn replicas(&self) -> Result<Vec<Replica>, String> {
        crate::replicas::of_installed(self.adapters.compose.as_ref(), self.adapters.api.as_ref())
    }
    fn report_replica_refused(&self, outcome: &Outcome) {
        self.adapters.cloud.report_replica_refused(outcome);
    }
    fn back_up(&self, dump: &[Replica]) -> Result<PathBuf, String> {
        self.adapters.database.back_up(dump)
    }
    fn save_package(&self, module: &str, version: &str) -> Result<(PathBuf, Vec<String>), String> {
        self.adapters.packages.save(module, version)
    }
    fn manifest(&self, package: &Path) -> Result<Vec<String>, String> {
        self.adapters.packages.manifest(package)
    }
    fn put_package(&self, module: &str, package: &Path, remove: &[String]) -> Result<(), String> {
        self.adapters.packages.put(module, package, remove)?;
        let mut order = self.put_order.borrow_mut();
        order.retain(|m| m != module);
        order.push(module.into());
        Ok(())
    }
    fn put_version(&self, version: &str, modules: &Modules) -> Result<(), String> {
        let order = std::mem::take(&mut *self.put_order.borrow_mut());
        self.modules_untold.set(true);
        self.adapters.compose.put_version(version, modules, &order)
    }
    /// Steps 4 to 6 of decision 0009's point 7 for `module`'s package, as
    /// `add` runs them, when its manifest asks for anything: its features
    /// checked, then its secrets, database and clients (the engine's state
    /// is the record: no progress file).
    fn provision(&self, module: &str) -> Result<(), String> {
        use crate::modules::provision;
        let compose = &self.adapters.compose;
        let Some(m) = compose.manifest(module)? else {
            return Ok(());
        };
        if !provision::provides_anything(&m) && m.permissions.is_none() {
            return Ok(());
        }
        let p = self.adapters.provision.as_ref();
        if m.oidc.is_some() || m.permissions.is_some() {
            provision::check_features(&m, &p.features()?)?;
        }
        if !provision::provides_anything(&m) {
            return Ok(());
        }
        let mut manifests = Vec::new();
        for name in compose.installed()?.modules.keys() {
            manifests.extend(compose.manifest(name)?);
        }
        let audiences = provision::audiences(&m, &manifests)?;
        provision::provision(p, &m, &audiences, &mut |_| Ok(()))
    }
    /// The installed version's modules started, then quality-data told
    /// which they are (after an install and after going back alike).
    fn start(&self) -> Result<(), String> {
        let started = self.adapters.compose.start();
        self.tell_modules();
        started
    }
    /// quality-data answers (the expected version, if one is given), then
    /// every module is healthy.
    fn healthy(&self, version: Option<&str>) -> Result<(), String> {
        let answering = self.adapters.api.version()?;
        if let Some(expected) = version {
            if answering != expected {
                return Err(format!("{answering} answers, not {expected}"));
            }
        }
        self.adapters.compose.modules_healthy()
    }
    /// Nothing connected while it is restored: every module is stopped
    /// first; the engine starts them after. A restore that fails leaves
    /// every module stopped (decision 0003 point 5): the database's own
    /// service, brought up to restore, is stopped again, nothing starts in
    /// a mixed state, and the engine reports it for a person.
    fn restore_database(&self, backup: &Path) -> Result<(), String> {
        self.adapters.compose.stop()?;
        let Err(failed) = self.adapters.database.restore(backup) else {
            return Ok(());
        };
        match self.adapters.compose.stop() {
            Ok(()) => Err(format!(
                "{failed}; every module is left stopped for a person to intervene"
            )),
            Err(stop) => Err(format!(
                "{failed}; stopping every module after it failed too ({stop})"
            )),
        }
    }
    fn install_commands(&self) -> Result<(), String> {
        self.adapters.packages.install_commands()
    }
}
