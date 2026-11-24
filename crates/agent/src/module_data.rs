//! What each installed module's manifest marks as critical data (decision
//! 0002, `data`), backed up beside the databases and checked, as
//! `tools/verify/module-data.sh` verified Compose's behaviour:
//!
//! - a **database**: must be one the PostgreSQL adapter dumped;
//! - a **volume**: `docker compose cp <service>:<path> -` into
//!   `modules/<module>/volumes/<name>.tar`, read back whole to its
//!   end-of-archive marker (an archive cut short lists without an error);
//! - a **key**: the file under `/opt/quality-data/<module>/` copied into
//!   `modules/<module>/keys/<path>` and compared.
//!
//! `modules.json` lists what was backed up, so a restore puts it back: a
//! key written whole; a volume **emptied first** (decision 0003 point 5),
//! then its archive copied into the stopped service's container, so it
//! holds the backup's files only. The volume is emptied by a throwaway
//! container of the agent's helper image (busybox, pinned by digest and
//! pulled with every version, so a module whose own image has no `find`
//! is restored too) with the stopped container's mounts (`docker run --rm
//! --volumes-from <container> --entrypoint find <helper> <path> -mindepth
//! 1 -delete`, as root, no network): it reaches the very volume the
//! container has, named or anonymous. A piece that is not critical is not
//! backed up, nor is a volume whose backup is a replica in sync (`backup:
//! replica`): the engine goes ahead only if the core says it is in sync,
//! and a restore leaves that volume as it is.

use std::io::Read;
use std::path::{Path, PathBuf};

use module_manifest::Manifest;
use serde::{Deserialize, Serialize};

use crate::command::{Cmd, Runner};
use crate::compose;

/// The list of what was backed up, in a backup's directory.
pub const LIST: &str = "modules.json";

/// One piece of a module's data in a backup; `file` is relative to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    Volume {
        module: String,
        service: String,
        path: String,
        file: String,
    },
    Key {
        module: String,
        path: String,
        file: String,
    },
}

/// The modules' data adapter.
pub struct ModuleData {
    /// `/opt/quality-data`.
    pub install: PathBuf,
    /// `/var/lib/quality-data/updater/installed.json`: which modules.
    pub record: PathBuf,
    pub runner: Box<dyn Runner>,
    /// The image that empties a volume (`<repo>@sha256:<digest>`).
    pub helper: String,
}

impl std::fmt::Debug for ModuleData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleData")
            .field("install", &self.install)
            .finish_non_exhaustive()
    }
}

/// The archive at `file` reads whole: whole blocks ending with two zero
/// ones, and every entry read to its end.
fn check_archive(file: &Path) -> Result<(), String> {
    let what = |e: std::io::Error| format!("{} does not read back whole: {e}", file.display());
    let bytes = std::fs::read(file).map_err(what)?;
    let marked = bytes.len() >= 1024
        && bytes.len() % 512 == 0
        && bytes[bytes.len() - 1024..].iter().all(|b| *b == 0);
    if !marked {
        return Err(format!(
            "{} does not end with the end-of-archive marker",
            file.display()
        ));
    }
    let mut archive = tar::Archive::new(bytes.as_slice());
    let mut entries = 0;
    for entry in archive.entries().map_err(what)? {
        std::io::copy(&mut entry.map_err(what)?, &mut std::io::sink()).map_err(what)?;
        entries += 1;
    }
    if entries == 0 {
        return Err(format!("{} holds nothing", file.display()));
    }
    Ok(())
}

impl ModuleData {
    /// The installation's, at `paths`, running the system's `docker` and
    /// emptying volumes with `helper`.
    pub fn new(paths: &crate::paths::Paths, helper: &str) -> Self {
        Self {
            install: paths.install_dir().to_path_buf(),
            record: paths.data_dir().join("updater/installed.json"),
            runner: Box::new(crate::command::System),
            helper: helper.into(),
        }
    }

    /// The installed modules that have a manifest, in their start order.
    fn manifests(&self) -> Result<Vec<Manifest>, String> {
        let record: compose::Record = match std::fs::read(&self.record) {
            Ok(b) => {
                serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", self.record.display()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{}: {e}", self.record.display())),
        };
        let mut all = Vec::new();
        for module in &record.now.order {
            all.extend(crate::manifests::read(&self.install, module)?);
        }
        Ok(all)
    }

    /// Every critical piece of the installed modules' data but the
    /// replicated volumes into `dir`,
    /// checked, then listed in [`LIST`]; `databases` are those dumped.
    pub fn back_up_into(&self, dir: &Path, databases: &[String]) -> Result<usize, String> {
        let mut pieces = Vec::new();
        for m in self.manifests()? {
            let data = m.data.dumped();
            for db in &data.databases {
                if !databases.contains(&db.name) {
                    return Err(format!(
                        "the database {} of {} is not on the server",
                        db.name, m.name
                    ));
                }
            }
            for v in &data.volumes {
                pieces.push(self.back_up_volume(dir, &m.name, v)?);
            }
            for k in &data.keys {
                pieces.push(self.back_up_key(dir, &m.name, &k.path)?);
            }
            // A generated secret that is critical is a key (decision 0009).
            for g in m.secrets.generate.iter().filter(|g| g.critical) {
                let key = format!("secrets/{}", g.name);
                pieces.push(self.back_up_key(dir, &m.name, &key)?);
            }
        }
        let json = serde_json::to_vec_pretty(&pieces).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(LIST), json).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(pieces.len())
    }

    pub(crate) fn back_up_volume(
        &self,
        dir: &Path,
        module: &str,
        v: &module_manifest::Volume,
    ) -> Result<Piece, String> {
        let file = format!("modules/{module}/volumes/{}.tar", v.name);
        let path = dir.join(&file);
        make_dirs(&path)?;
        let source = format!("{}:{}", v.service, v.path);
        let cmd = compose::command(&self.install, module, &["cp", &source, "-"])?.stdout(&path);
        self.runner
            .run(&cmd)
            .map_err(|e| format!("{module}: the volume {}: {e}", v.name))?;
        check_archive(&path)?;
        Ok(Piece::Volume {
            module: module.into(),
            service: v.service.clone(),
            path: v.path.clone(),
            file,
        })
    }

    pub(crate) fn back_up_key(&self, dir: &Path, module: &str, key: &str) -> Result<Piece, String> {
        let source = self.install.join(module).join(key);
        let file = format!("modules/{module}/keys/{key}");
        let copy = dir.join(&file);
        make_dirs(&copy)?;
        let bytes = read_file(&source)?;
        write_private(&copy, &bytes)?;
        if read_file(&copy)? != bytes {
            return Err(format!("{} does not read back as it was", copy.display()));
        }
        Ok(Piece::Key {
            module: module.into(),
            path: key.into(),
            file,
        })
    }

    /// Every piece [`LIST`] names put back (a backup without one has none).
    pub fn restore(&self, backup: &Path) -> Result<(), String> {
        let list = backup.join(LIST);
        let pieces: Vec<Piece> = match std::fs::read(&list) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", list.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("{}: {e}", list.display())),
        };
        for piece in &pieces {
            match piece {
                Piece::Volume {
                    module,
                    service,
                    path,
                    file,
                } => self
                    .restore_volume(module, service, path, &backup.join(file))
                    .map_err(|e| format!("restoring {module} {path}: {e}"))?,
                Piece::Key { module, path, file } => {
                    let bytes = read_file(&backup.join(file))?;
                    let target = self.install.join(module).join(path);
                    make_dirs(&target)?;
                    write_private(&target, &bytes)?;
                    self.give_secret(module, path, &target)?;
                }
            }
        }
        tracing::warn!(backup = %backup.display(), pieces = pieces.len(), "module data restored");
        Ok(())
    }
}

impl ModuleData {
    /// A restored file the module's `secrets.generate` names, as the agent
    /// makes it: mode `0400`, owned by `secrets.uid` when the agent runs as
    /// root. Any other key keeps `0600`.
    fn give_secret(&self, module: &str, key: &str, file: &Path) -> Result<(), String> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let Some(m) = crate::manifests::read(&self.install, module)? else {
            return Ok(());
        };
        let generated = m
            .secrets
            .generate
            .iter()
            .any(|g| key == format!("secrets/{}", g.name));
        if !generated {
            return Ok(());
        }
        let what = |e: std::io::Error| format!("{}: {e}", file.display());
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o400)).map_err(what)?;
        if std::fs::metadata(file).map_err(what)?.uid() == 0 {
            std::os::unix::fs::chown(file, Some(m.secrets.uid), None).map_err(what)?;
        }
        Ok(())
    }

    /// The volume at `path` of `service`'s stopped container emptied, then
    /// `archive` extracted into it.
    fn restore_volume(
        &self,
        module: &str,
        service: &str,
        path: &str,
        archive: &Path,
    ) -> Result<(), String> {
        let ps = compose::command(&self.install, module, &["ps", "--all", "--quiet", service])?;
        let out = self.runner.run(&ps)?;
        let container = out
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .ok_or_else(|| format!("{service} has no container"))?;
        let inspect = Cmd::new("docker", ["inspect", container]);
        let mounts = mounts(&self.runner.run(&inspect)?)?;
        if !mounts.iter().any(|m| m == path) {
            return Err(format!("{path} is not a volume of {service}'s container"));
        }
        let empty = Cmd::new(
            "docker",
            [
                "run",
                "--rm",
                "--network",
                "none",
                "--user",
                "0:0",
                "--volumes-from",
                container,
                "--entrypoint",
                "find",
                &self.helper,
                path,
                "-mindepth",
                "1",
                "-delete",
            ],
        );
        self.runner
            .run(&empty)
            .map_err(|e| format!("emptying it: {e}"))?;
        let parent = Path::new(path).parent().unwrap_or(Path::new("/"));
        let target = format!("{service}:{}", parent.display());
        let cp = compose::command(&self.install, module, &["cp", "-", &target])?.stdin(archive);
        self.runner.run(&cp)?;
        Ok(())
    }
}

/// Where a container's mounts are, from `docker inspect`.
fn mounts(inspect: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value =
        serde_json::from_str(inspect).map_err(|e| format!("docker inspect does not read: {e}"))?;
    let all = v[0]["Mounts"]
        .as_array()
        .ok_or("docker inspect names no container")?;
    Ok(all
        .iter()
        .filter_map(|m| m["Destination"].as_str().map(str::to_string))
        .collect())
}

fn make_dirs(file: &Path) -> Result<(), String> {
    let parent = file.parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(bytes)
}

/// `bytes` to `path` (`0600`), whole: a temporary file, synced, renamed.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let partial = path.with_extension("partial");
    let write = || -> std::io::Result<()> {
        let _ = std::fs::remove_file(&partial);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&partial)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&partial, path)
    };
    write().map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[path = "module_data_tests.rs"]
mod tests;
