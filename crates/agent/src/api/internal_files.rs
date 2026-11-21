//! The agent's files for the `internal` entry point
//! (`/etc/quality-data/internal/`): the installation's CA, the agent's
//! certificate and its private key (decision 0003 point 4).
//!
//! A renewal replaces the three together, so that a crash never leaves a
//! key and a certificate that do not match. Each new file is first written
//! whole beside its old one as `<name>.new` (a temporary file, synced,
//! renamed), the private key last; then each `.new` is renamed over its
//! file, the private key last. So `private-key.pem.new` exists only once
//! every new file is whole: [`InternalFiles::finish_pending`] then finishes
//! the renames, and otherwise removes what a cut left staged.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use x509_cert::der::DecodePem;
use x509_cert::Certificate;

use crate::paths::Paths;

/// The three files, in the order they are replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalFiles {
    pub ca: PathBuf,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
}

/// New contents for the three files, PEM.
#[derive(Clone, PartialEq, Eq)]
pub struct Contents {
    pub ca: String,
    pub certificate: String,
    pub private_key: String,
}

/// The key is never printed.
impl std::fmt::Debug for Contents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contents").finish_non_exhaustive()
    }
}

fn staged(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".new");
    PathBuf::from(name)
}

fn io(file: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{}: {e}", file.display())
}

/// `bytes` whole at `path` with `mode`: a temporary file, synced, renamed.
fn write_whole(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let _ = std::fs::remove_file(&partial);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&partial)
        .map_err(io(&partial))?;
    // Exactly `mode`, whatever the umask.
    f.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(io(&partial))?;
    f.write_all(bytes).map_err(io(&partial))?;
    f.sync_all().map_err(io(&partial))?;
    std::fs::rename(&partial, path).map_err(io(path))
}

impl InternalFiles {
    pub fn new(paths: &Paths) -> Self {
        Self {
            ca: paths.internal_ca(),
            certificate: paths.internal_certificate(),
            private_key: paths.internal_private_key(),
        }
    }

    fn all(&self) -> [&Path; 3] {
        [&self.ca, &self.certificate, &self.private_key]
    }

    fn sync_dir(&self) -> Result<(), String> {
        let dir = self.private_key.parent().unwrap_or(Path::new("/"));
        std::fs::File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(io(dir))
    }

    /// Replace the three files together (see the module's comment).
    pub fn replace(&self, new: &Contents) -> Result<(), String> {
        self.finish_pending()?;
        write_whole(&staged(&self.ca), new.ca.as_bytes(), 0o644)?;
        write_whole(
            &staged(&self.certificate),
            new.certificate.as_bytes(),
            0o644,
        )?;
        write_whole(
            &staged(&self.private_key),
            new.private_key.as_bytes(),
            0o600,
        )?;
        self.sync_dir()?;
        self.finish_pending()
    }

    /// Finish a replacement a cut left half done, or drop one that was not
    /// staged whole.
    pub fn finish_pending(&self) -> Result<(), String> {
        let whole = staged(&self.private_key).exists();
        let mut changed = false;
        for file in self.all() {
            let new = staged(file);
            if !new.exists() {
                continue;
            }
            if whole {
                std::fs::rename(&new, file).map_err(io(file))?;
            } else {
                std::fs::remove_file(&new).map_err(io(&new))?;
            }
            changed = true;
        }
        if changed {
            self.sync_dir()?;
        }
        Ok(())
    }

    /// When the agent's certificate (the first of its file) ends.
    pub fn not_after(&self) -> Result<DateTime<Utc>, String> {
        let pem = std::fs::read(&self.certificate).map_err(io(&self.certificate))?;
        not_after(&pem).map_err(|e| format!("{}: {e}", self.certificate.display()))
    }
}

/// When the first certificate of `pem` ends.
pub fn not_after(pem: &[u8]) -> Result<DateTime<Utc>, String> {
    let cert = Certificate::from_pem(first_certificate(pem)?)
        .map_err(|e| format!("not a PEM certificate: {e}"))?;
    let end = cert.tbs_certificate.validity.not_after.to_unix_duration();
    let secs = i64::try_from(end.as_secs()).map_err(|e| e.to_string())?;
    DateTime::from_timestamp(secs, 0).ok_or_else(|| "a date out of range".to_string())
}

/// The first `CERTIFICATE` block of a PEM file (a chain has several).
fn first_certificate(pem: &[u8]) -> Result<&[u8], String> {
    const END: &[u8] = b"-----END CERTIFICATE-----";
    let start = find(pem, b"-----BEGIN CERTIFICATE-----").ok_or("no PEM certificate")?;
    let end = find(&pem[start..], END).ok_or("a PEM certificate without its end")?;
    Ok(&pem[start..start + end + END.len()])
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
#[path = "internal_files_tests.rs"]
mod tests;
