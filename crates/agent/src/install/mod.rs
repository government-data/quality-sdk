//! `quality-data-agent install`: the first platform version of an
//! installation (decision 0004 point 2), as steps that survive a crash, its
//! progress in `updater/install.json` ([`progress`]):
//!
//! 1. **Choose**: the version from the verified index (`--version`, or the
//!    newest that reaches the installation).
//! 2. **Prepare**: every package downloaded and checked, every image
//!    pulled, and only then each package put and the version put (which
//!    writes `installed.json`).
//! 3. **Start**: the modules started.
//! 4. **Verify**: once the internal certificate exists, quality-data
//!    answers the version, every module is healthy, quality-data is told
//!    the installed modules, and `install.json` is removed. A call that
//!    quality-data does not answer (no connection, or a TLS handshake that
//!    fails) is tried again for up to [`ANSWER_WITHIN`]: Traefik may not
//!    have loaded the internal entry point's files the installer has just
//!    written, and serves another certificate until it does. An answer that
//!    is wrong (an HTTP error, another version) fails at once.
//!
//! The install is reported once it is done, and when it fails after a
//! version was chosen (decision 0004).
//!
//! It stops twice (decision 0006), each time for the installer to run a
//! module's own commands and run it again: after prepare, before anything
//! starts ([`Done::Prepared`]), and after the start while the internal
//! certificate is missing ([`Done::NeedsCertificate`]). No backup and no drain:
//! there is no data yet. It refuses an installation installed before
//! (`installed.json` without `install.json`); a failure leaves the modules
//! as they are and says the step it stopped at.

use std::path::{Path, PathBuf};
use std::time::Duration;

use updater::engine::{apply_order, CloudError, Offer, OfferedPackage};
use updater::state::Modules;

pub mod adapters;
mod progress;

pub use progress::{Progress, Step};

/// What `install` works through: the adapters that update an installation.
pub trait Installer {
    /// The verified index's version: `version`, or with none the newest
    /// not paused whose rollout reaches the installation; with the
    /// registry's credential to pull its images.
    fn choose(&self, version: Option<&str>) -> Result<Offer, CloudError>;
    /// One package of the offer, downloaded and checked by its SHA-256.
    fn download_package(&self, offer: &Offer, package: &OfferedPackage) -> Result<PathBuf, String>;
    /// Every image of the offer by digest, and the agent's helper image.
    fn pull_images(&self, offer: &Offer) -> Result<(), String>;
    fn put_package(&self, module: &str, package: &Path) -> Result<(), String>;
    /// Put the version (its `IMAGE_*` lines and `installed.json`), its
    /// modules starting in `order`.
    fn put_version(&self, version: &str, modules: &Modules, order: &[String])
        -> Result<(), String>;
    fn start(&self) -> Result<(), String>;
    /// Whether the agent's certificate for the `internal` entry point
    /// exists.
    fn internal_certificate(&self) -> bool;
    /// quality-data answers `version`, then every module's health.
    fn healthy(&self, version: &str) -> Result<(), CallError>;
    /// `PUT /updater/v1/modules`.
    fn tell_modules(&self) -> Result<(), CallError>;
    /// Wait `d` before a call is tried again.
    fn wait(&self, d: Duration);
    /// A report of kind `install`, or with `failed` (where it stopped and
    /// why) of kind `install_failed`; if it cannot be written, it does not
    /// matter.
    fn report(&self, version: &str, failed: Option<&str>);
}

/// Why a step that calls quality-data's `updater/v1` failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// quality-data did not answer: no connection, or its TLS handshake
    /// failed. Tried again for up to [`ANSWER_WITHIN`].
    NoAnswer(String),
    /// Anything else: an HTTP error answer, another version, a module not
    /// healthy. Not tried again.
    Failed(String),
}

/// How long the verify step tries again a call quality-data does not
/// answer, [`ANSWER_EVERY`] apart, before it fails.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(60);
/// The wait between two tries of a call quality-data did not answer.
pub const ANSWER_EVERY: Duration = Duration::from_secs(5);

/// How a run of `install` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    /// Prepared, nothing started: run the module's first-start commands,
    /// then run it again.
    Prepared { version: String },
    /// Installed and answering.
    Installed { version: String },
    /// Started; the internal certificate is missing. Run it again once it
    /// is issued.
    NeedsCertificate { version: String },
}

/// The installation's files `install` looks at.
#[derive(Debug, Clone)]
pub struct Files {
    /// `updater/installed.json`.
    pub installed: PathBuf,
    /// `updater/install.json`.
    pub progress: PathBuf,
}

/// Install, or resume an install, up to where it can go now.
pub fn install(i: &dyn Installer, files: &Files, version: Option<&str>) -> Result<Done, String> {
    let (p, offer) = match Progress::load(&files.progress)? {
        Some(p) => {
            if let Some(v) = version.filter(|v| *v != p.version) {
                return Err(format!(
                    "an install of {} is in progress, not of {v}: run it again without --version",
                    p.version
                ));
            }
            (p, None)
        }
        None if files.installed.exists() => {
            return Err(format!(
                "{} exists: this installation was installed before; it is updated by \
                 `quality-data-agent run`, not installed again",
                files.installed.display()
            ));
        }
        None => {
            let o = i
                .choose(version)
                .map_err(|e| stopped("choose", &cloud(e)))?;
            let p = Progress::new(&o.version);
            p.save(&files.progress)?;
            (p, Some(o))
        }
    };
    let version = p.version.clone();
    let done = steps(i, files, p, offer);
    match &done {
        Ok(Done::Installed { .. }) => i.report(&version, None),
        Ok(Done::Prepared { .. } | Done::NeedsCertificate { .. }) => {}
        Err(err) => i.report(&version, Some(err)),
    }
    done
}

/// The steps from the stored one, each stored before the next.
fn steps(
    i: &dyn Installer,
    files: &Files,
    mut p: Progress,
    mut offer: Option<Offer>,
) -> Result<Done, String> {
    loop {
        match p.step {
            Step::Prepare => {
                let o = match offer.take() {
                    Some(o) => o,
                    None => i
                        .choose(Some(&p.version))
                        .map_err(|e| stopped("prepare", &cloud(e)))?,
                };
                prepare(i, &o).map_err(|e| stopped("prepare", &e))?;
                p.step = Step::Prepared;
                p.save(&files.progress)?;
                return Ok(Done::Prepared { version: p.version });
            }
            Step::Prepared => p.step = Step::Start,
            Step::Start => {
                i.start().map_err(|e| stopped("start", &e))?;
                p.step = Step::Verify;
            }
            Step::Verify => {
                if !i.internal_certificate() {
                    return Ok(Done::NeedsCertificate { version: p.version });
                }
                verify(i, &p.version).map_err(|e| stopped("verify", &e))?;
                Progress::remove(&files.progress)?;
                return Ok(Done::Installed { version: p.version });
            }
        }
        p.save(&files.progress)?;
    }
}

/// Everything fetched and checked before anything is put.
fn prepare(i: &dyn Installer, o: &Offer) -> Result<(), String> {
    let order = apply_order(&o.packages)?;
    let downloaded = order
        .iter()
        .map(|p| Ok((p.module.as_str(), i.download_package(o, p)?)))
        .collect::<Result<Vec<_>, String>>()?;
    i.pull_images(o)?;
    for (module, package) in &downloaded {
        i.put_package(module, package)?;
    }
    let order: Vec<String> = order.into_iter().map(|p| p.module).collect();
    i.put_version(&o.version, &o.modules, &order)
}

fn verify(i: &dyn Installer, version: &str) -> Result<(), String> {
    until_answered(i, &|| i.healthy(version))?;
    until_answered(i, &|| i.tell_modules())
}

/// `call`, tried again while quality-data does not answer it, for up to
/// [`ANSWER_WITHIN`]; any other failure at once.
fn until_answered(
    i: &dyn Installer,
    call: &dyn Fn() -> Result<(), CallError>,
) -> Result<(), String> {
    let mut waited = Duration::ZERO;
    loop {
        match call() {
            Ok(()) => return Ok(()),
            Err(CallError::NoAnswer(e)) if waited < ANSWER_WITHIN => {
                tracing::info!(
                    "no answer yet, trying again in {}s: {e}",
                    ANSWER_EVERY.as_secs()
                );
                i.wait(ANSWER_EVERY);
                waited += ANSWER_EVERY;
            }
            Err(CallError::NoAnswer(e) | CallError::Failed(e)) => return Err(e),
        }
    }
}

fn cloud(e: CloudError) -> String {
    match e {
        CloudError::Rejected(m) => format!("the cloud refused: {m}"),
        CloudError::Network(m) => format!("the cloud did not answer: {m}"),
    }
}

/// Where it stopped and why; the modules are left as they are.
fn stopped(step: &str, why: &str) -> String {
    format!("the install stopped at {step}: {why}; run it again to resume")
}

#[cfg(test)]
mod tests;
