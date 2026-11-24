//! A module's secret files (decision 0009, point 2, as V1 verified):
//!
//! - **The folder** `/opt/quality-data/<module>/secrets/`, owner root, mode
//!   `0700`, beside the module's `.env`, never in a version folder.
//! - **Each file** written aside (`.<name>.tmp`, `0400`), synced, renamed,
//!   then given to the manifest's `secrets.uid` with mode `0400`. A file
//!   that exists is never replaced: a crash anywhere converges on the next
//!   run with the same value.
//! - **Rotation** writes `<name>.next` first (reused if a crash left it)
//!   and renames it over the file later.
//! - **Content**: random bytes from the operating system, base64url without
//!   padding and without a newline.
//! - **`.env`** gets only what is not secret, the `QD_*` keys the agent
//!   owns ([`AGENT_KEYS`]); every other line is kept.
//! - **The installation's CA** (public) copied to `<module>/ca.crt`
//!   (`0444`) for a module with a database.
//!
//! No value reaches a log, an error or a command line: errors name the
//! file and the step; [`Secret`] prints as `[secret]`.

use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use base64::Engine;
use rand_core::RngCore;

/// The keys of a module's `.env` the agent writes (and rewrites).
pub const AGENT_KEYS: &[&str] = &[
    "QD_SECRETS_DIR",
    "QD_OIDC_ISSUER",
    "QD_OIDC_PEOPLE_CLIENT_ID",
    "QD_OIDC_MACHINE_CLIENT_ID",
    "QD_DB_HOST",
    "QD_DB_PORT",
    "QD_DB_USER",
    "QD_DB_NAME",
    "QD_DB_CA_FILE",
];

/// A secret's value: never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[secret]")
    }
}

impl Secret {
    /// The value, for a request body or a verifier only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// The modules' secret files under `/opt/quality-data`.
#[derive(Debug, Clone)]
pub struct ModuleSecrets {
    install: PathBuf,
    /// The installation's CA certificate (`Paths::internal_ca`).
    ca: PathBuf,
}

/// `bytes` random bytes, base64url without padding.
fn random(bytes: u32) -> String {
    let mut buf = vec![0u8; bytes as usize];
    rand_core::OsRng.fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

fn io<'a>(path: &'a Path, step: &'a str) -> impl FnOnce(std::io::Error) -> String + 'a {
    move |e| format!("{} ({step}): {e}", path.display())
}

/// The user the agent runs as: the owner of a folder it just made.
fn own_uid(made: &Path) -> Result<u32, String> {
    Ok(std::fs::metadata(made)
        .map_err(io(made, "read its owner"))?
        .uid())
}

impl ModuleSecrets {
    pub fn new(paths: &crate::paths::Paths) -> Self {
        Self {
            install: paths.install_dir().to_path_buf(),
            ca: paths.internal_ca(),
        }
    }

    /// `/opt/quality-data/<module>/secrets`.
    pub fn dir(&self, module: &str) -> PathBuf {
        self.install.join(module).join("secrets")
    }

    /// The folder, made if absent, mode `0700` (owner root: the agent).
    pub fn ensure_dir(&self, module: &str) -> Result<PathBuf, String> {
        let dir = self.dir(module);
        std::fs::create_dir_all(&dir).map_err(io(&dir, "make the secrets folder"))?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(io(&dir, "set the folder's mode"))?;
        Ok(dir)
    }

    /// The file `name` with `bytes` random bytes, unless it exists; then
    /// owned by `uid`, mode `0400`. Whether it was made now.
    pub fn ensure(&self, module: &str, name: &str, bytes: u32, uid: u32) -> Result<bool, String> {
        let dir = self.ensure_dir(module)?;
        let file = dir.join(name);
        let made = !file.exists();
        if made {
            let tmp = dir.join(format!(".{name}.tmp"));
            write_new(&tmp, random(bytes).as_bytes())?;
            std::fs::rename(&tmp, &file).map_err(io(&file, "rename into place"))?;
        }
        self.give(&dir, &file, uid)?;
        Ok(made)
    }

    /// `<name>.next` with a new value, unless a crash left one: it is
    /// reused, never replaced.
    pub fn stage_next(&self, module: &str, name: &str, bytes: u32) -> Result<(), String> {
        let dir = self.ensure_dir(module)?;
        let next = dir.join(format!("{name}.next"));
        if next.exists() {
            return Ok(());
        }
        let tmp = dir.join(format!(".{name}.next.tmp"));
        write_new(&tmp, random(bytes).as_bytes())?;
        std::fs::rename(&tmp, &next).map_err(io(&next, "rename into place"))
    }

    /// `<name>.next`'s value, when there is one.
    pub fn read_next(&self, module: &str, name: &str) -> Result<Option<Secret>, String> {
        let next = self.dir(module).join(format!("{name}.next"));
        if !next.exists() {
            return Ok(None);
        }
        self.read_path(&next).map(Some)
    }

    /// `<name>.next` renamed over `<name>`, owned by `uid`, `0400`; nothing
    /// to do when it was renamed already.
    pub fn promote_next(&self, module: &str, name: &str, uid: u32) -> Result<(), String> {
        let dir = self.dir(module);
        let next = dir.join(format!("{name}.next"));
        let file = dir.join(name);
        if next.exists() {
            std::fs::rename(&next, &file).map_err(io(&file, "rename the new value"))?;
        }
        self.give(&dir, &file, uid)
    }

    /// The value of `name`.
    pub fn read(&self, module: &str, name: &str) -> Result<Secret, String> {
        self.read_path(&self.dir(module).join(name))
    }

    fn read_path(&self, path: &Path) -> Result<Secret, String> {
        let text = std::fs::read_to_string(path).map_err(io(path, "read"))?;
        Ok(Secret(text))
    }

    /// `file` owned by `uid`, mode `0400`. Only root gives a file to
    /// another user: as any other user, `uid` must be its own.
    fn give(&self, dir: &Path, file: &Path, uid: u32) -> Result<(), String> {
        let me = own_uid(dir)?;
        if me == 0 {
            std::os::unix::fs::chown(file, Some(uid), None).map_err(io(file, "set its owner"))?;
        } else if me != uid {
            return Err(format!(
                "{}: only root gives a file to uid {uid}",
                file.display()
            ));
        }
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o400))
            .map_err(io(file, "set its mode"))
    }

    /// The module's `.env` with the agent's `QD_*` lines set to `vars` (in
    /// their order, after the lines it keeps); written whole, `0600`.
    pub fn write_env(&self, module: &str, vars: &[(&str, String)]) -> Result<(), String> {
        for (k, v) in vars {
            if !AGENT_KEYS.contains(k) || v.contains(['\n', '\r']) {
                return Err(format!("{module}: {k} cannot go in its .env"));
            }
        }
        let env = self.install.join(module).join(".env");
        let text = match std::fs::read_to_string(&env) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io(&env, "read")(e)),
        };
        let mut out: Vec<&str> = text
            .lines()
            .filter(|l| {
                let key = l.split('=').next().unwrap_or_default().trim();
                !AGENT_KEYS.contains(&key)
            })
            .collect();
        while out.last().is_some_and(|l| l.trim().is_empty()) {
            out.pop();
        }
        let mut text = out.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        for (k, v) in vars {
            text.push_str(&format!("{k}={v}\n"));
        }
        let tmp = self.install.join(module).join(".env.partial");
        let _ = std::fs::remove_file(&tmp);
        write_mode(&tmp, text.as_bytes(), 0o600)?;
        std::fs::rename(&tmp, &env).map_err(io(&env, "rename into place"))
    }

    /// The installation's CA certificate copied to `<module>/ca.crt`,
    /// `0444`; its path.
    pub fn copy_ca(&self, module: &str) -> Result<PathBuf, String> {
        let bytes = std::fs::read(&self.ca).map_err(io(&self.ca, "read the CA"))?;
        let target = self.install.join(module).join("ca.crt");
        let tmp = self.install.join(module).join(".ca.crt.tmp");
        let _ = std::fs::remove_file(&tmp);
        write_mode(&tmp, &bytes, 0o444)?;
        std::fs::rename(&tmp, &target).map_err(io(&target, "rename into place"))?;
        Ok(target)
    }
}

/// A new file at `path` (any left by a crash removed first), mode `0400`,
/// synced.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    write_mode(path, bytes, 0o400)
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(io(path, "create"))?;
    // The mode as asked, whatever the umask.
    f.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(io(path, "set its mode"))?;
    f.write_all(bytes).map_err(io(path, "write"))?;
    f.sync_all().map_err(io(path, "sync"))
}

#[cfg(test)]
#[path = "module_secrets_tests.rs"]
mod tests;
