//! `updater/install.json`: the version being installed and the step to do
//! next, written before each step that changes something, so an install
//! cut halfway resumes there. Its presence is also what lets `install` run
//! where `installed.json` already exists.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The next step of an install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Prepare,
    Start,
    Verify,
}

/// `install.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub version: String,
    pub step: Step,
}

impl Progress {
    /// A chosen version, to prepare.
    pub fn new(version: &str) -> Self {
        Self {
            version: version.into(),
            step: Step::Prepare,
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

    /// Written to a temporary file and renamed, as the state file is.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let dir = path.parent().unwrap_or(Path::new("."));
        let temporary = path.with_extension("json.tmp");
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(dir)?;
            let mut f = std::fs::File::create(&temporary)?;
            f.write_all(&serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
            f.sync_all()?;
            std::fs::rename(&temporary, path)?;
            std::fs::File::open(dir)?.sync_all()
        };
        write().map_err(|e| format!("could not save {}: {e}", path.display()))
    }

    /// The install is done.
    pub fn remove(path: &Path) -> Result<(), String> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("could not remove {}: {e}", path.display())),
        }
    }
}
