//! The package files adapter (decision 0003): each module's package
//! extracted under `/opt/quality-data`, and the copy of the installed one
//! under `/var/lib/quality-data/updater/releases`.
//!
//! ```text
//! /opt/quality-data/<module>/<version>/   a package's files, as signed
//! /opt/quality-data/<module>/current      -> <version>
//! /opt/quality-data/<module>/.env, ...    the module's own, never touched
//! ```
//!
//! A package is extracted into a directory of its own and `current` is
//! switched to it with one rename, so the module's files are always one
//! whole version. The version `current` pointed to before is kept beside
//! it, to go back to; older ones are removed. What a package may hold is
//! [`crate::package_archive`]'s.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::host::Packages;
use crate::package_archive::{extract, files_under, listing, pack};

/// The space an update needs on each of its disks.
pub const MINIMUM_FREE: u64 = 2 << 30;

/// The free bytes of the disk a path is on.
pub type FreeBytes = Box<dyn Fn(&Path) -> Result<u64, String>>;

/// The package files of an installation.
pub struct PackageFiles {
    /// `/opt/quality-data`.
    pub install: PathBuf,
    /// `/var/lib/quality-data/updater/releases`.
    pub releases: PathBuf,
    /// The disks an update writes to: the installed files, the downloads
    /// and the backups.
    pub disks: Vec<PathBuf>,
    pub free_bytes: FreeBytes,
}

impl std::fmt::Debug for PackageFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackageFiles")
            .field("install", &self.install)
            .field("releases", &self.releases)
            .finish_non_exhaustive()
    }
}

impl PackageFiles {
    /// The installation's, at `paths`, with `df` telling the free space.
    pub fn new(paths: &crate::paths::Paths) -> Self {
        Self {
            install: paths.install_dir().to_path_buf(),
            releases: paths.releases_dir(),
            disks: vec![
                paths.install_dir().to_path_buf(),
                paths.data_dir().to_path_buf(),
            ],
            free_bytes: Box::new(df),
        }
    }

    fn module_dir(&self, module: &str) -> Result<PathBuf, String> {
        safe_name(module)?;
        Ok(self.install.join(module))
    }

    /// The version `current` points to, if any.
    fn current(&self, module: &str) -> Result<Option<String>, String> {
        let link = self.module_dir(module)?.join("current");
        match std::fs::read_link(&link) {
            Ok(target) => Ok(target.to_str().map(str::to_string)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", link.display())),
        }
    }
}

/// A module or version name that is one plain path component.
fn safe_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'));
    if ok && name != "current" {
        Ok(())
    } else {
        Err(format!("{name:?} cannot name a directory"))
    }
}

/// The version a package file is of: `<version>.tar.gz`, as the AWS
/// adapter and [`Packages::save`] name them.
fn version_of(package: &Path) -> Result<String, String> {
    let name = package.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let version = name
        .strip_suffix(".tar.gz")
        .ok_or_else(|| format!("{} is not a <version>.tar.gz", package.display()))?;
    safe_name(version)?;
    Ok(version.to_string())
}

/// The directory's own entry written to disk (after a rename in it).
fn sync_dir(dir: &Path) -> Result<(), String> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("{}: {e}", dir.display()))
}

fn remove_dir(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("{}: {e}", dir.display()))
        }
        _ => Ok(()),
    }
}

impl Packages for PackageFiles {
    fn free_space(&self) -> Result<(), String> {
        for disk in &self.disks {
            std::fs::create_dir_all(disk).map_err(|e| format!("{}: {e}", disk.display()))?;
            let free = (self.free_bytes)(disk)?;
            if free < MINIMUM_FREE {
                return Err(format!(
                    "{} MB free on {}: an update needs {} MB",
                    free >> 20,
                    disk.display(),
                    MINIMUM_FREE >> 20
                ));
            }
        }
        Ok(())
    }

    /// A copy of the installed files of `module` at `version`, as a
    /// package, and its files.
    fn save(&self, module: &str, version: &str) -> Result<(PathBuf, Vec<String>), String> {
        safe_name(version)?;
        if self.current(module)?.as_deref() != Some(version) {
            return Err(format!(
                "the installed files of {module} are not {version}'s"
            ));
        }
        let dir = self.module_dir(module)?.join(version);
        let to = self.releases.join(module).join(format!("{version}.tar.gz"));
        let files = pack(&dir, &to)?;
        Ok((to, files))
    }

    fn manifest(&self, package: &Path) -> Result<Vec<String>, String> {
        listing(package)
    }

    /// `package` becomes `module`'s files. `remove` needs no work: a
    /// version's files are a directory of their own, so what the previous
    /// package had and this one has not is not there.
    fn put(&self, module: &str, package: &Path, _remove: &[String]) -> Result<(), String> {
        let version = version_of(package)?;
        let files = listing(package)?;
        let module_dir = self.module_dir(module)?;
        let dir = module_dir.join(&version);
        // Going back to a version kept whole needs no extraction.
        if !dir.is_dir() || files_under(&dir)? != files {
            let partial = module_dir.join(format!(".{version}.partial"));
            remove_dir(&partial)?;
            extract(package, &partial)?;
            remove_dir(&dir)?;
            std::fs::rename(&partial, &dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let previous = self.current(module)?;
        let next = module_dir.join(".current.next");
        let _ = std::fs::remove_file(&next);
        std::os::unix::fs::symlink(&version, &next)
            .and_then(|()| std::fs::rename(&next, module_dir.join("current")))
            .map_err(|e| format!("{}: {e}", module_dir.join("current").display()))?;
        sync_dir(&module_dir)?;
        // Keep this version and the one before it; a name that is not a
        // version (the module's own files) is never removed.
        let keep = [Some(version), previous];
        for entry in std::fs::read_dir(&module_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let old = semver::Version::parse(&name).is_ok() && !keep.contains(&Some(name.clone()));
            if old && entry.file_type().is_ok_and(|t| t.is_dir()) {
                remove_dir(&entry.path())?;
            }
        }
        Ok(())
    }

    /// No package carries commands yet; where they would be installed is
    /// not decided (decision 0003).
    fn install_commands(&self) -> Result<(), String> {
        Ok(())
    }
}

/// The free bytes of `path`'s disk, from `df -Pk` (POSIX output).
fn df(path: &Path) -> Result<u64, String> {
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(path)
        .output()
        .map_err(|e| format!("could not run df: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "df {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_df(&String::from_utf8_lossy(&out.stdout))
}

/// The available kilobytes of `df -Pk`'s one line, in bytes.
fn parse_df(output: &str) -> Result<u64, String> {
    output
        .lines()
        .nth(1)
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|kb| kb.parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .ok_or_else(|| format!("df answered {output:?}"))
}

#[cfg(test)]
#[path = "package_files_tests.rs"]
mod tests;
