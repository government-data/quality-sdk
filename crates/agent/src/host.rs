//! The real [`Host`] of the update engine, composed of one adapter per
//! concern (decision 0003), each behind its own trait so it is tested on its
//! own and the composition is tested with fakes:
//!
//! | Adapter | `Host` methods |
//! | --- | --- |
//! | [`Cloud`] (AWS) | `check_cloud`, `report_to_cloud`, `download_package`, `pull_images` |
//! | [`Compose`] (Docker Compose) | `put_version`, `start`, `installed`, the module health of `healthy` |
//! | [`Database`] (PostgreSQL) | `back_up`, `restore_database` |
//! | [`Packages`] (package files) | `free_space`, `save_package`, `manifest`, `put_package`, `install_commands` |
//! | [`Api`] (quality-data's `updater/v1`) | telling `save`, `orders`, `maintenance`, `drain`, the version of `healthy` |
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
use std::collections::BTreeSet;

use module_manifest::{Manifest, Permissions};
use serde::Serialize;
use updater::engine::{CloudError, Drain, Host, Installed, Offer, OfferedPackage};
use updater::policy::Orders;
use updater::state::{Modules, Outcome, State};

/// Where versions come from: the AWS side (decision 0001).
pub trait Cloud {
    fn check(&self, installed_version: &str) -> Result<Offer, CloudError>;
    fn report(&self, outcome: &Outcome);
    fn download_package(&self, offer: &Offer, package: &OfferedPackage) -> Result<PathBuf, String>;
    fn pull_images(&self, offer: &Offer) -> Result<(), String>;
}

/// The running containers.
pub trait Compose {
    /// Put `version` with its modules' images; `order` is the order its
    /// packages were put in, the order its modules start in.
    fn put_version(&self, version: &str, modules: &Modules, order: &[String])
        -> Result<(), String>;
    fn start(&self) -> Result<(), String>;
    /// Every module stopped: nothing connected while the database is
    /// restored.
    fn stop(&self) -> Result<(), String>;
    fn installed(&self) -> Result<Installed, String>;
    /// Every installed module's health.
    fn modules_healthy(&self) -> Result<(), String>;
    /// The installed module's manifest (none: its package has none).
    fn manifest(&self, module: &str) -> Result<Option<Manifest>, String>;
}

/// The database: back up, check the backup, restore.
pub trait Database {
    fn back_up(&self) -> Result<PathBuf, String>;
    fn restore(&self, backup: &Path) -> Result<(), String>;
}

/// The package files under `/opt/quality-data` and `/var/lib/quality-data`.
pub trait Packages {
    fn free_space(&self) -> Result<(), String>;
    fn save(&self, module: &str, version: &str) -> Result<(PathBuf, Vec<String>), String>;
    fn manifest(&self, package: &Path) -> Result<Vec<String>, String>;
    fn put(&self, module: &str, package: &Path, remove: &[String]) -> Result<(), String>;
    fn install_commands(&self) -> Result<(), String>;
}

/// quality-data's API, through `updater/v1`.
pub trait Api {
    /// `PUT /updater/v1/state`.
    fn put_state(&self, state: &State) -> Result<(), String>;
    /// `GET /updater/v1/orders`: approval and no request when it does not
    /// answer.
    fn orders(&self) -> Orders;
    /// `PUT /updater/v1/maintenance`.
    fn maintenance(&self, reason: Option<&str>, lasts: Duration) -> Result<(), String>;
    /// `GET /updater/v1/drain`.
    fn drain(&self, horizon: Duration) -> Result<Drain, String>;
    /// `GET /updater/v1/version`: the version of quality-data that answers.
    fn version(&self) -> Result<String, String>;
    /// `PUT /updater/v1/modules`: every installed module, the whole list.
    fn put_modules(&self, modules: &[InstalledModule]) -> Result<(), String>;
    /// The `features` of `GET /updater/v1/version`: what the core
    /// understands beyond the first calls (decision 0009).
    fn features(&self) -> Result<Features, String>;
}

/// What a core understands beyond the first `updater/v1` calls (decision
/// 0009, point 6); a core that names none has none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Features(pub BTreeSet<String>);

impl Features {
    /// `PUT …/modules/{module}/clients` and `…/clients/disable`.
    pub const MODULE_CLIENTS: &'static str = "module-clients";
    /// `menu.requires` in `PUT /updater/v1/modules`, checked by the apps
    /// menu.
    pub const MENU_REQUIRES: &'static str = "menu-requires";
    /// `permissions` in `PUT /updater/v1/modules`.
    pub const MODULE_PERMISSIONS: &'static str = "module-permissions";
    /// `POST /updater/v1/audit`.
    pub const AGENT_AUDIT: &'static str = "agent-audit";

    pub fn has(&self, feature: &str) -> bool {
        self.0.contains(feature)
    }
}

/// An installed module as `PUT /updater/v1/modules` names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledModule {
    pub name: String,
    pub version: String,
    /// Its manifest's `drain.path`, which the module serves on its route of
    /// the `internal` entry point (`null`: no work to drain).
    pub drain_path: Option<String>,
    /// Its route's path on the web entry point (`null`: no route).
    pub route_path: Option<String>,
    /// Its entry in the core's apps menu (`null`: none).
    pub menu: Option<ModuleMenu>,
    /// Its actions and managed policies, to a core that has
    /// `module-permissions` only (absent otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Permissions>,
}

/// A module's menu entry as `PUT /updater/v1/modules` names it: its label
/// and the page it opens, in English and Spanish (decision 0002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleMenu {
    pub label: InLanguages,
    pub path: InLanguages,
    /// The action a person must hold to see it (absent: everyone).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires: Option<String>,
}

/// A text in each language the web speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InLanguages {
    pub en: String,
    pub es: String,
}

impl InstalledModule {
    /// `name` at `version`, with what its manifest declares that `features`
    /// let the core understand.
    pub fn new(
        name: &str,
        version: &str,
        manifest: Option<&Manifest>,
        features: &Features,
    ) -> Self {
        let route = manifest.and_then(|m| m.route.as_ref());
        // A menu shown only to who holds an action goes to a core that
        // checks it, or not at all: hidden from everyone, never shown to
        // everyone (decision 0009, point 6).
        let menu = manifest
            .and_then(|m| m.menu.as_ref())
            .filter(|menu| menu.requires.is_none() || features.has(Features::MENU_REQUIRES))
            .zip(route)
            .map(|(menu, route)| {
                let path = menu.paths(route);
                ModuleMenu {
                    label: InLanguages {
                        en: menu.label.en.clone(),
                        es: menu.label.es.clone(),
                    },
                    path: InLanguages {
                        en: path.en,
                        es: path.es,
                    },
                    requires: menu.requires.clone(),
                }
            });
        Self {
            name: name.into(),
            version: version.into(),
            drain_path: manifest
                .and_then(|m| m.drain.as_ref())
                .map(|d| d.path.clone()),
            route_path: route.map(|r| r.path.clone()),
            menu,
            permissions: manifest
                .and_then(|m| m.permissions.clone())
                .filter(|_| features.has(Features::MODULE_PERMISSIONS)),
        }
    }
}

/// The time: the system's, or a fake one in tests.
pub trait Clock {
    fn now(&self) -> DateTime<Utc>;
    fn wait(&self, d: Duration);
}

/// The system's clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
    fn wait(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// Every adapter the host is made of.
pub struct Adapters {
    pub cloud: Box<dyn Cloud>,
    pub compose: Box<dyn Compose>,
    pub database: Box<dyn Database>,
    pub packages: Box<dyn Packages>,
    pub api: Box<dyn Api>,
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
}

impl AgentHost {
    pub fn new(adapters: Adapters, state_file: PathBuf) -> Self {
        Self {
            adapters,
            state_file,
            put_order: RefCell::default(),
            modules_untold: Cell::new(true),
        }
    }

    /// Tell quality-data every installed module (`PUT /updater/v1/modules`,
    /// the whole list, so telling it twice changes nothing). A failure is
    /// logged and told again by [`AgentHost::tell_modules_if_untold`].
    pub fn tell_modules(&self) {
        match self.put_modules() {
            Ok(()) => self.modules_untold.set(false),
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
        compose.installed().and_then(|i| {
            let modules = i
                .modules
                .iter()
                .map(|(name, m)| {
                    let manifest = compose.manifest(name)?;
                    Ok(InstalledModule::new(
                        name,
                        &m.version,
                        manifest.as_ref(),
                        &features,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            self.adapters.api.put_modules(&modules)
        })
    }

    /// [`AgentHost::tell_modules`] when quality-data has not been told them
    /// since the agent started or the last version was put (or telling it
    /// failed): the service does it before each round.
    pub fn tell_modules_if_untold(&self) {
        if self.modules_untold.get() {
            self.tell_modules();
        }
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
    fn back_up(&self) -> Result<PathBuf, String> {
        self.adapters.database.back_up()
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
