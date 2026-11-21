//! The daemon's state, on disk: which phase an update is in, and what is
//! needed to finish or undo it if the process (or the machine) stops
//! halfway. It is written **before** each step that changes something, to a
//! temporary file that is then renamed: it is never left half written.
//!
//! quality-data's API reads the same document (`POST /internal/report`), so
//! its field and phase names are part of the protocol with the API.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

mod file;

/// A module at a platform version (decision 0003).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Module {
    pub version: String,
    /// Its images by name, each pinned by digest
    /// (`<repository>@sha256:<hex>`).
    pub images: BTreeMap<String, String>,
}

/// Every module of a platform version, by its name. The engine names none:
/// it passes them to the host.
pub type Modules = BTreeMap<String, Module>;

/// A module's package of a target: downloaded and checked against its
/// SHA-256 (decision 0003: one package per module).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModulePackage {
    pub module: String,
    pub path: PathBuf,
}

/// A module's installed package, copied to go back to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPackage {
    pub module: String,
    pub path: PathBuf,
    /// Its files (the ones `manifest` lists).
    pub manifest: Vec<String>,
}

/// A version that can be installed: what the cloud offered, already
/// downloaded and checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "file::TargetFile", into = "file::TargetFile")]
pub struct Target {
    /// The platform version.
    pub version: String,
    pub modules: Modules,
    /// Every module's package, in the order they are applied: a module
    /// after those that provide what it requires.
    pub packages: Vec<ModulePackage>,
    #[serde(default)]
    pub notes: String,
}

/// The version installed before applying: to go back to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "file::PreviousFile", into = "file::PreviousFile")]
pub struct Previous {
    /// The platform version.
    pub version: String,
    pub modules: Modules,
    /// A copy of every installed module's package.
    pub packages: Vec<SavedPackage>,
}

impl Previous {
    /// The files `module`'s installed package had (none if it had none).
    pub fn manifest_of(&self, module: &str) -> &[String] {
        self.packages
            .iter()
            .find(|p| p.module == module)
            .map_or(&[], |p| p.manifest.as_slice())
    }
}

/// Where it is. Each phase carries what is needed to go on or undo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// Nothing to do (or nothing new).
    Idle,
    /// A new version downloaded and checked, waiting for its moment.
    Prepared { target: Target },
    /// In maintenance, waiting for what runs to finish.
    Draining {
        target: Target,
        since: DateTime<Utc>,
        /// The "Update now" that started it, if one did.
        request: Option<String>,
    },
    /// Backing up the database. Nothing changed yet.
    BackingUp {
        target: Target,
        request: Option<String>,
    },
    /// Changing the package and the images.
    Applying {
        target: Target,
        previous: Previous,
        backup: PathBuf,
    },
    /// Waiting for the new version to answer.
    Verifying {
        target: Target,
        previous: Previous,
        backup: PathBuf,
    },
    /// The new one did not work: going back to the previous one on the same
    /// database.
    RollingBack {
        target: Target,
        previous: Previous,
        backup: PathBuf,
        reason: String,
    },
    /// The previous one did not start either: restoring the database backup.
    Restoring {
        target: Target,
        previous: Previous,
        backup: PathBuf,
        reason: String,
    },
    /// Nothing worked: a person is needed (and acknowledges it once solved).
    Critical {
        target: Target,
        previous: Previous,
        backup: PathBuf,
        reason: String,
    },
}

impl Phase {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Prepared { .. } => "prepared",
            Self::Draining { .. } => "draining",
            Self::BackingUp { .. } => "backing_up",
            Self::Applying { .. } => "applying",
            Self::Verifying { .. } => "verifying",
            Self::RollingBack { .. } => "rolling_back",
            Self::Restoring { .. } => "restoring",
            Self::Critical { .. } => "critical",
        }
    }

    /// The version it is going to, if any.
    pub fn target(&self) -> Option<&Target> {
        match self {
            Self::Idle => None,
            Self::Prepared { target }
            | Self::Draining { target, .. }
            | Self::BackingUp { target, .. }
            | Self::Applying { target, .. }
            | Self::Verifying { target, .. }
            | Self::RollingBack { target, .. }
            | Self::Restoring { target, .. }
            | Self::Critical { target, .. } => Some(target),
        }
    }

    /// The installation was already touched (and it has to be finished or
    /// undone).
    pub fn in_progress(&self) -> bool {
        matches!(
            self,
            Self::Applying { .. }
                | Self::Verifying { .. }
                | Self::RollingBack { .. }
                | Self::Restoring { .. }
        )
    }
}

/// How the last attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    /// Updated and answering.
    Updated,
    /// The new one did not answer and it went back to the previous one.
    RolledBack,
    /// Nor did the previous one: the backup was restored.
    Restored,
    /// It could not go back: a person is needed.
    Critical,
    /// It did not start: work was running, or the window ended.
    Postponed,
    /// It could not be prepared or backed up (nothing was touched).
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub kind: OutcomeKind,
    /// The version attempted.
    pub version: String,
    /// From which.
    pub from: String,
    pub detail: String,
    pub at: DateTime<Utc>,
}

/// What is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(flatten)]
    pub phase: Phase,
    /// The installed version.
    pub installed_version: String,
    /// The last version the cloud offered (if it is not the installed one).
    #[serde(default)]
    pub available: Option<Available>,
    /// The versions that rolled back here: they are not retried by themselves.
    #[serde(default)]
    pub failed: Vec<String>,
    #[serde(default)]
    pub last: Option<Outcome>,
    /// The last "Update now" handled (so as not to handle it twice).
    #[serde(default)]
    pub handled_request: Option<String>,
    #[serde(default)]
    pub last_check: Option<DateTime<Utc>>,
    /// What the cloud said last if it was not a version (refused, no
    /// connection…).
    #[serde(default)]
    pub warning: Option<String>,
    /// Since when it is in the current phase (so the web shows how long).
    #[serde(default)]
    pub phase_since: Option<DateTime<Utc>>,
    /// What happens inside the phase, in words ("2 flows running").
    #[serde(default)]
    pub progress: Option<String>,
}

/// A version the cloud offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Available {
    pub version: String,
    #[serde(default)]
    pub notes: String,
}

impl State {
    pub fn new(installed_version: &str) -> Self {
        Self {
            phase: Phase::Idle,
            installed_version: installed_version.into(),
            available: None,
            failed: Vec::new(),
            last: None,
            handled_request: None,
            last_check: None,
            warning: None,
            phase_since: None,
            progress: None,
        }
    }

    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("{} is damaged: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("could not read {}: {e}", path.display())),
        }
    }

    /// Writes to a temporary file, flushes it to disk and renames it: if the
    /// power goes off, the previous state or the new one is left, never one
    /// half written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let temporary = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let write = || -> std::io::Result<()> {
            let mut f = std::fs::File::create(&temporary)?;
            f.write_all(&json)?;
            f.sync_all()?;
            std::fs::rename(&temporary, path)?;
            std::fs::File::open(dir)?.sync_all()
        };
        write().map_err(|e| format!("could not save {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn modules(version: &str) -> Modules {
        let module = |images: &[(&str, &str)]| Module {
            version: version.into(),
            images: images
                .iter()
                .map(|(name, repo)| (name.to_string(), format!("{repo}:{version}@sha256:a")))
                .collect(),
        };
        Modules::from([
            (
                "quality-data".into(),
                module(&[("api", "r/api"), ("web", "r/web")]),
            ),
            ("orchestrator".into(), module(&[("server", "r/orch")])),
        ])
    }

    fn target() -> Target {
        Target {
            version: "1.3.0".into(),
            modules: modules("1.3.0"),
            packages: vec![ModulePackage {
                module: "quality-data".into(),
                path: "/var/lib/quality-data/updater/packages/1.3.0.tar.gz".into(),
            }],
            notes: String::new(),
        }
    }

    #[test]
    fn it_is_saved_and_read_back_the_same() {
        let dir = std::env::temp_dir().join(format!("updater-state-{}", std::process::id()));
        let path = dir.join("state.json");
        assert_eq!(State::load(&path).unwrap(), None);
        let mut s = State::new("1.2.0");
        s.phase = Phase::Applying {
            target: target(),
            previous: Previous {
                version: "1.2.0".into(),
                modules: modules("1.2.0"),
                packages: vec![SavedPackage {
                    module: "orchestrator".into(),
                    path: "/x/orchestrator-1.2.0.tar".into(),
                    manifest: vec!["compose.yml".into()],
                }],
            },
            backup: "/var/lib/quality-data/backups/quality-data-1".into(),
        };
        s.failed.push("1.2.5".into());
        s.save(&path).unwrap();
        assert_eq!(State::load(&path).unwrap(), Some(s));
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_phase_is_read_by_name() {
        let mut s = State::new("1.2.0");
        s.phase = Phase::Prepared { target: target() };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["phase"], "prepared");
        assert_eq!(json["target"]["version"], "1.3.0");
        assert_eq!(s.phase.name(), "prepared");
        assert!(!s.phase.in_progress());
    }
}
