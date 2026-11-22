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
//!    the installed modules, and `install.json` is removed.
//!
//! The install is reported once it is done, and when it fails after a
//! version was chosen (decision 0004).
//!
//! It runs in two phases: without the internal certificate it stops after
//! the start ([`Done::NeedsCertificate`]), so the installer can issue it
//! against the started database and run it again. No backup and no drain:
//! there is no data yet. It refuses an installation installed before
//! (`installed.json` without `install.json`); a failure leaves the modules
//! as they are and says the step it stopped at.

use std::path::{Path, PathBuf};

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
    fn healthy(&self, version: &str) -> Result<(), String>;
    /// `PUT /updater/v1/modules`.
    fn tell_modules(&self) -> Result<(), String>;
    /// A report of kind `install`, or with `failed` (where it stopped and
    /// why) of kind `install_failed`; if it cannot be written, it does not
    /// matter.
    fn report(&self, version: &str, failed: Option<&str>);
}

/// How a run of `install` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
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
        Ok(Done::NeedsCertificate { .. }) => {}
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
                p.step = Step::Start;
            }
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
    i.healthy(version)?;
    i.tell_modules()
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
