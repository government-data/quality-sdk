//! The ports of the agent's [`crate::host::AgentHost`]: one trait per
//! concern (decision 0003), each implemented by an adapter and faked in
//! tests.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};

use module_manifest::Manifest;
use updater::engine::{CloudError, Drain, Installed, Offer, OfferedPackage, Replica, ReplicaState};
use updater::state::{Modules, Outcome, State};

use crate::installed_module::{Features, InstalledModule};
use crate::module_events::ModuleEvents;
use crate::module_roles::ModuleRole;
use updater::policy::Orders;

/// Where versions come from: the AWS side (decision 0001).
pub trait Cloud {
    fn check(&self, installed_version: &str) -> Result<Offer, CloudError>;
    fn report(&self, outcome: &Outcome);
    /// An update refused because a replica that backs up a critical volume
    /// is not in sync, reported as `replica_refused`.
    fn report_replica_refused(&self, outcome: &Outcome);
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
    /// The installed module's machine roles with their images (decision
    /// 0013), from its `compose.yml` and `.env`.
    fn roles(&self, _module: &str, _manifest: &Manifest) -> Vec<ModuleRole> {
        Vec::new()
    }
    /// The installed module's events with their schemas (decision 0017),
    /// read from its package.
    fn events(&self, _module: &str, _manifest: &Manifest) -> Option<ModuleEvents> {
        None
    }
}

/// The database: back up, check the backup, restore.
pub trait Database {
    /// The databases and the modules' critical data, and the replicated
    /// volumes in `dump` (those with no replica, decision 0012).
    fn back_up(&self, dump: &[Replica]) -> Result<PathBuf, String>;
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
    /// `PUT /updater/v1/modules/{module}/role-secrets`: the values of its
    /// roles' declared secrets, never logged (decision 0013).
    fn put_role_secrets(
        &self,
        module: &str,
        secrets: &crate::role_secrets::RoleSecrets,
    ) -> Result<(), String>;
    /// `GET /updater/v1/modules/{module}/replica`: what the core says of
    /// the replica of the installed module's data.
    fn replica(&self, module: &str) -> Result<ReplicaState, String>;
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
