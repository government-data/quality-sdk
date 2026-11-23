//! The Docker Compose adapter (decision 0003): each module's `.env` with its
//! images, the record of what is installed, and `docker compose` per
//! module, as `tools/verify/compose.sh` verified it.
//!
//! - **Images**: the module's `/opt/quality-data/<module>/.env` gets one
//!   `IMAGE_<NAME>=<repo>@sha256:<digest>` per image (the name upper-cased,
//!   `-` as `_`); its other lines stay as they are. The package's
//!   `compose.yml` references `${IMAGE_…}` and stays as signed.
//! - **Each module is one Compose project** named after the module (never
//!   after its directory, so a new version keeps its volumes), run in
//!   `<module>/current` with its `.env`.
//! - **What is installed** is recorded in
//!   `/var/lib/quality-data/updater/installed.json`: the platform version,
//!   its modules, the order they start in (a module after those it
//!   requires) and the one it replaced, to go back to in its order.
//! - **Routes**: once every module is up, each installed module's Traefik
//!   route is written from its manifest and the others' removed
//!   ([`crate::routes::sync`]).

use std::path::{Path, PathBuf};

use updater::engine::Installed;
use updater::state::Modules;

use crate::command::{Cmd, Runner};

mod health;
mod record;
mod run;

pub use record::{Put, Record};

/// The Docker Compose adapter.
pub struct DockerCompose {
    /// `/opt/quality-data`.
    pub install: PathBuf,
    /// `/var/lib/quality-data/updater/installed.json`.
    pub record: PathBuf,
    /// Where the modules' Traefik routes go ([`crate::routes`]).
    pub routes: PathBuf,
    pub runner: Box<dyn Runner>,
    /// How long `up --wait` waits for the healthchecks.
    pub wait_secs: u64,
}

impl std::fmt::Debug for DockerCompose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerCompose")
            .field("install", &self.install)
            .finish_non_exhaustive()
    }
}

/// `IMAGE_<NAME>` for an image named `name`.
pub fn image_variable(name: &str) -> Result<String, String> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return Err(format!("the image name {name:?} cannot name a variable"));
    }
    Ok(format!(
        "IMAGE_{}",
        name.to_ascii_uppercase().replace('-', "_")
    ))
}

fn check_module(module: &str) -> Result<(), String> {
    let ok = !module.is_empty()
        && !module.starts_with(['.', '-'])
        && module
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'));
    if ok {
        Ok(())
    } else {
        Err(format!("{module:?} cannot name a Compose project"))
    }
}

/// `text` (a `.env`) with every `IMAGE_…` line replaced by `images`, in
/// their order, after the lines it keeps.
fn with_images(text: &str, images: &[(String, String)]) -> String {
    let mut out: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("IMAGE_"))
        .collect();
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    let mut text = out.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    for (k, v) in images {
        text.push_str(&format!("{k}={v}\n"));
    }
    text
}

/// Write `bytes` to `path` whole: a temporary file, synced, renamed.
fn write_whole(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let partial = path.with_extension("partial");
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&partial);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&partial)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&partial, path)
    };
    write().map_err(|e| format!("{}: {e}", path.display()))
}

impl DockerCompose {
    /// The installation's, at `paths`, running the system's `docker`.
    pub fn new(paths: &crate::paths::Paths) -> Self {
        Self {
            install: paths.install_dir().to_path_buf(),
            record: paths.installed_record(),
            routes: paths.routes_dir(),
            runner: Box::new(crate::command::System),
            wait_secs: 300,
        }
    }

    /// `docker compose` for `module`, then `args`.
    pub fn compose(&self, module: &str, args: &[&str]) -> Result<Cmd, String> {
        command(&self.install, module, args)
    }

    /// Run `docker compose` for `module`.
    pub fn run(&self, module: &str, args: &[&str]) -> Result<String, String> {
        self.runner.run(&self.compose(module, args)?)
    }
}

/// `docker compose` for `module` installed under `install`, then `args`.
pub fn command(install: &Path, module: &str, args: &[&str]) -> Result<Cmd, String> {
    check_module(module)?;
    let dir = install.join(module);
    let current = dir.join("current");
    let mut all = vec![
        "compose".to_string(),
        "--project-name".into(),
        module.into(),
        "--project-directory".into(),
        current.display().to_string(),
        "--file".into(),
        current.join("compose.yml").display().to_string(),
        "--env-file".into(),
        dir.join(".env").display().to_string(),
    ];
    all.extend(args.iter().map(|a| (*a).to_string()));
    Ok(Cmd::new("docker", all))
}

impl DockerCompose {
    /// The record, if any version was put.
    pub fn read_record(&self) -> Result<Option<Record>, String> {
        match std::fs::read(&self.record) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("{}: {e}", self.record.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", self.record.display())),
        }
    }

    /// Each module's `.env` with the images of `modules`, then the record:
    /// `version` starts in `order` (the order its packages were put), or,
    /// when it is the version the record replaced, in that version's order.
    pub fn put_version(
        &self,
        version: &str,
        modules: &Modules,
        order: &[String],
    ) -> Result<(), String> {
        for (module, m) in modules {
            check_module(module)?;
            let mut images = Vec::with_capacity(m.images.len());
            for (name, image) in &m.images {
                if image.is_empty()
                    || image.contains(|c: char| c.is_whitespace() || "$\"'`\\".contains(c))
                {
                    return Err(format!("{module}/{name}: {image:?} cannot go in a .env"));
                }
                images.push((image_variable(name)?, image.clone()));
            }
            let env = self.install.join(module).join(".env");
            let text = match std::fs::read_to_string(&env) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(format!("{}: {e}", env.display())),
            };
            write_whole(&env, with_images(&text, &images).as_bytes(), 0o600)?;
        }
        let record = record::next(self.read_record()?, version, modules, order);
        let json = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
        write_whole(&self.record, &json, 0o644)
    }

    /// What is installed: the record's version and modules.
    pub fn installed(&self) -> Result<Installed, String> {
        let r = self.read_record()?.ok_or_else(|| {
            format!(
                "no platform version is recorded in {}",
                self.record.display()
            )
        })?;
        Ok(Installed {
            version: r.now.version,
            modules: r.now.modules,
        })
    }
}

#[cfg(test)]
mod tests;
