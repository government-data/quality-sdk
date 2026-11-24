//! An update's state machine.
//!
//! Each step looks at the stored phase, does **one** thing and stores the
//! next phase before going on. What touches the world (the cloud, the API,
//! Docker, the files) goes through [`Host`], so each case is tested without
//! Docker.
//!
//! The order, and why:
//!
//! 1. **Prepare** (outside the window, without touching what runs): download
//!    the package and check it, pull the images by digest, check the space.
//! 2. **Wait for the moment**: an "Update now", or the automatic window.
//! 3. **Drain**: maintenance; wait until nothing is running. If it does not
//!    empty in time it is postponed without touching anything.
//! 4. **Back up** and check the backup. If it fails, nothing is touched. A
//!    volume backed up by a replica is not copied: the update goes ahead
//!    only if that replica is in sync, asked here and before maintenance.
//! 5. **Apply**: keep the current package, put the new one, bring it up.
//! 6. **Verify** that the expected version answers.
//! 7. If not: **go back** to the previous package on the same database (the
//!    migrations only add); if that fails too, **restore** the backup; if
//!    that fails too, **critical**, and it warns.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::policy::Orders;
use crate::state::{Modules, Outcome, OutcomeKind, Phase, State};

mod apply;
mod drain;
mod order;
mod prepare;
mod recover;
mod replica;

use apply::{apply, verify};
use drain::{back_up, drain};
pub use order::apply_order;
use prepare::waiting;
use recover::{restore, roll_back};
pub use replica::{Replica, ReplicaState};

/// What the cloud offers this installation: a platform version and its
/// modules (decision 0003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub version: String,
    pub modules: Modules,
    /// One package per module.
    pub packages: Vec<OfferedPackage>,
    pub notes: String,
    /// The registry's temporary credential (host, user, token).
    pub registry: Option<(String, String, String)>,
}

/// A module's package, as the cloud offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferedPackage {
    pub module: String,
    /// Where the host fetches it from (the release bucket's key).
    pub key: String,
    /// Its SHA-256, which the host checks before keeping it.
    pub sha256: String,
    /// The capabilities the module provides and requires: a module is
    /// applied after those that provide what it requires.
    pub provides: Vec<String>,
    pub requires: Vec<String>,
}

/// Why the cloud did not offer a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudError {
    /// It said no: it warns and does not retry right away.
    Rejected(String),
    /// It could not be reached: it is retried.
    Network(String),
}

/// What still runs (from `GET /internal/drain`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Drain {
    pub drained: bool,
    pub running: u64,
    pub deploying: u64,
    pub scheduled_soon: u64,
}

/// What is installed now: the platform version and its modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: String,
    pub modules: Modules,
}

/// The world: the cloud, the API, Docker and the files.
pub trait Host {
    fn now(&self) -> DateTime<Utc>;
    fn wait(&self, d: Duration);
    /// Store the state (and tell the API, if it answers).
    fn save(&self, s: &State) -> Result<(), String>;

    fn check_cloud(&self, installed_version: &str) -> Result<Offer, CloudError>;
    /// Tell the cloud how it ended (to brake a version that fails in several
    /// installations). If it does not answer, it does not matter.
    fn report_to_cloud(&self, outcome: &Outcome);
    /// Tell the cloud an update was refused because a replica that backs
    /// up a critical volume is not in sync (`replica_refused`). If it does
    /// not answer, it does not matter.
    fn report_replica_refused(&self, outcome: &Outcome);

    fn free_space(&self) -> Result<(), String>;
    /// Download one module's package of `offer` and check its SHA-256;
    /// returns where it was left.
    fn download_package(&self, offer: &Offer, package: &OfferedPackage) -> Result<PathBuf, String>;
    /// Pull the images by digest (with the offer's credential).
    fn pull_images(&self, offer: &Offer) -> Result<(), String>;

    /// The API's policy and request. Without the API (or an old one),
    /// approval and no request: nothing is done by itself.
    fn orders(&self) -> Orders;
    /// `Some(reason)` enters (or renews) maintenance for `lasts`; `None`
    /// leaves it.
    fn maintenance(&self, reason: Option<&str>, lasts: Duration) -> Result<(), String>;
    fn drain(&self, horizon: Duration) -> Result<Drain, String>;

    fn installed(&self) -> Result<Installed, String>;
    /// Every installed module's critical volume whose backup is a replica,
    /// with what the core says of it (none: nothing is backed up that way).
    fn replicas(&self) -> Result<Vec<Replica>, String>;
    /// Back up the database and the keys, and check the backup. A volume
    /// whose backup is a replica is not copied.
    fn back_up(&self) -> Result<PathBuf, String>;
    /// Copy the files of `module`'s installed package.
    fn save_package(&self, module: &str, version: &str) -> Result<(PathBuf, Vec<String>), String>;
    /// The files a package carries.
    fn manifest(&self, package: &Path) -> Result<Vec<String>, String>;
    /// Remove `remove` from `module`'s files and put `package`'s.
    fn put_package(&self, module: &str, package: &Path, remove: &[String]) -> Result<(), String>;
    /// Put `version` with every one of its modules' images.
    fn put_version(&self, version: &str, modules: &Modules) -> Result<(), String>;
    /// What `module`'s new package asks for (its secret files, database
    /// and OIDC clients, decision 0009) in place before it starts; running
    /// it twice changes nothing.
    fn provision(&self, module: &str) -> Result<(), String>;
    fn start(&self) -> Result<(), String>;
    /// That it answers; with `version`, that it is that one.
    fn healthy(&self, version: Option<&str>) -> Result<(), String>;
    fn restore_database(&self, backup: &Path) -> Result<(), String>;
    /// The new package's commands, in the system.
    fn install_commands(&self) -> Result<(), String>;
}

/// Who asks for the step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    /// The service: it follows the policy, the window and the failed versions.
    Service,
    /// A person on the server, now.
    Person,
}

/// How often the cloud is asked.
pub const CHECK_EVERY: Duration = Duration::from_secs(3600);
/// How long maintenance is renewed on each long step.
const MAINTENANCE: Duration = Duration::from_secs(2 * 3600);
/// How often to look whether what runs finished.
const DRAIN_POLL: Duration = Duration::from_secs(10);

/// `X.Y.Z` as numbers (`None` if it is not a version).
fn semver(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().trim_start_matches('v').split('.');
    let n = |p: Option<&str>| p?.parse::<u64>().ok();
    let version = (n(parts.next())?, n(parts.next())?, n(parts.next())?);
    parts.next().is_none().then_some(version)
}

/// Whether `offered` is newer than `current`. An installation with a version
/// that does not parse (a development one) takes any other published version.
pub fn is_newer(offered: &str, current: &str) -> bool {
    match (semver(offered), semver(current)) {
        (Some(o), Some(c)) => o > c,
        (Some(_), None) => offered != current,
        (None, _) => false,
    }
}

fn difference(a: &[String], b: &[String]) -> Vec<String> {
    a.iter().filter(|x| !b.contains(x)).cloned().collect()
}

/// One step. Returns whether to go on at once (the phase changed to one with
/// something to do) or the next round can wait.
pub fn step(host: &dyn Host, s: &mut State, who: Who) -> Result<bool, String> {
    let now = host.now();
    let phase = s.phase.clone();
    match phase {
        Phase::Idle | Phase::Prepared { .. } => waiting(host, s, who, now),
        Phase::Draining {
            target,
            since,
            request,
        } => drain(host, s, who, target, since, request),
        Phase::BackingUp { target, request } => back_up(host, s, target, request),
        Phase::Applying {
            target,
            previous,
            backup,
        } => apply(host, s, target, previous, backup),
        Phase::Verifying {
            target,
            previous,
            backup,
        } => verify(host, s, target, previous, backup),
        Phase::RollingBack {
            target,
            previous,
            backup,
            reason,
        } => roll_back(host, s, target, previous, backup, reason),
        Phase::Restoring {
            target,
            previous,
            backup,
            reason,
        } => restore(host, s, target, previous, backup, reason),
        Phase::Critical { .. } => Ok(false),
    }
}

fn enter(host: &dyn Host, s: &mut State, phase: Phase) -> Result<bool, String> {
    s.phase = phase;
    s.phase_since = Some(host.now());
    s.progress = None;
    host.save(s)?;
    Ok(true)
}

fn finish(host: &dyn Host, s: &mut State, outcome: Outcome) -> Result<(), String> {
    let terminal = matches!(
        outcome.kind,
        OutcomeKind::Updated
            | OutcomeKind::RolledBack
            | OutcomeKind::Restored
            | OutcomeKind::Critical
    );
    let (version, kind, detail) = (&outcome.version, outcome.kind, &outcome.detail);
    match log_level(kind) {
        tracing::Level::ERROR => tracing::error!(version = %version, kind = ?kind, "{detail}"),
        tracing::Level::WARN => tracing::warn!(version = %version, kind = ?kind, "{detail}"),
        _ => tracing::info!(version = %version, kind = ?kind, "{detail}"),
    }
    s.last = Some(outcome.clone());
    s.phase_since = Some(host.now());
    s.progress = None;
    host.save(s)?;
    if terminal {
        host.report_to_cloud(&outcome);
    }
    Ok(())
}

/// The level an outcome is logged at: a success or a postponement is
/// information; going back, a failure or a refusal is a warning; needing a
/// person is an error.
fn log_level(kind: OutcomeKind) -> tracing::Level {
    match kind {
        OutcomeKind::Updated | OutcomeKind::Postponed => tracing::Level::INFO,
        OutcomeKind::RolledBack | OutcomeKind::Restored | OutcomeKind::Failed => {
            tracing::Level::WARN
        }
        OutcomeKind::Critical => tracing::Level::ERROR,
    }
}

fn outcome(
    s: &State,
    kind: OutcomeKind,
    version: &str,
    detail: String,
    at: DateTime<Utc>,
) -> Outcome {
    Outcome {
        kind,
        version: version.into(),
        from: s.installed_version.clone(),
        detail,
        at,
    }
}

/// Leave maintenance, insisting a little: the API may be finishing its
/// start-up. If it cannot, maintenance expires by itself.
fn leave_maintenance(host: &dyn Host) {
    for _ in 0..5 {
        match host.maintenance(None, Duration::ZERO) {
            Ok(()) => return,
            Err(e) => {
                tracing::warn!("could not leave maintenance: {e}");
                host.wait(Duration::from_secs(5));
            }
        }
    }
}

/// Go on until there is nothing left to do now.
pub fn advance(host: &dyn Host, s: &mut State, who: Who) -> Result<(), String> {
    // A cap in case something loops back on itself: no path has this many
    // phases.
    for _ in 0..20 {
        if !step(host, s, who)? {
            return Ok(());
        }
    }
    Err("the update did not advance after 20 steps".into())
}

#[cfg(test)]
mod tests;
