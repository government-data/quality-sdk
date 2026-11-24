//! A purge's backup and deletion on the installation (decision 0009, point
//! 8): what [`super::provisioner::Provisioner`] does for `purge_backup` and
//! `purge_files`.
//!
//! The backup folder `/var/lib/quality-data/backups/purge/<module>-<time>/`
//! (`0700`) holds each database the agent created (`<db>.dump`, read back
//! whole as an update's backup is), each volume (`modules/<module>/volumes/
//! <name>.tar`, through a container created for it, the module being
//! down), each key and every file of the secrets folder
//! (`modules/<module>/keys/…`), then `complete`. It is never pruned.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use module_manifest::Manifest;

use crate::command::Cmd;
use crate::compose;
use crate::module_data::ModuleData;
use crate::paths::Paths;
use crate::postgres::Postgres;

/// The backup of `m` into a new folder; its path. A critical piece that
/// fails removes the folder and fails; another one is a warning.
pub(crate) fn back_up(
    paths: &Paths,
    postgres: &Postgres,
    data: &ModuleData,
    m: &Manifest,
    warnings: &mut Vec<String>,
) -> Result<PathBuf, String> {
    let root = paths.backups_dir().join("purge");
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let time = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
    let dir = root.join(format!("{}-{time}", m.name));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let done = pieces(paths, postgres, data, m, &dir, warnings).and_then(|()| {
        std::fs::write(dir.join("complete"), "").map_err(|e| format!("{}: {e}", dir.display()))
    });
    if let Err(err) = done {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!(
            "the purge's backup failed, nothing was deleted: {err}"
        ));
    }
    Ok(dir)
}

fn pieces(
    paths: &Paths,
    postgres: &Postgres,
    data: &ModuleData,
    m: &Manifest,
    dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    for db in m.created_databases() {
        postgres.dump_checked(&db.name, &dir.join(format!("{}.dump", db.name)))?;
    }
    let install = paths.install_dir();
    for v in &m.data.volumes {
        let create = compose::command(install, &m.name, &["create", "--no-recreate", &v.service])?;
        let backed = postgres
            .runner
            .run(&create)
            .and_then(|_| data.back_up_volume(dir, &m.name, v));
        let _ = postgres.runner.run(&compose::command(
            install,
            &m.name,
            &["down", "--remove-orphans"],
        )?);
        match backed {
            Ok(_) => {}
            Err(e) if v.critical => return Err(e),
            Err(e) => warnings.push(format!("the volume {} was not backed up: {e}", v.name)),
        }
    }
    for k in &m.data.keys {
        match data.back_up_key(dir, &m.name, &k.path) {
            Ok(_) => {}
            Err(e) if k.critical => return Err(e),
            Err(e) => warnings.push(format!("the key {} was not backed up: {e}", k.path)),
        }
    }
    let secrets = install.join(&m.name).join("secrets");
    if let Ok(entries) = std::fs::read_dir(&secrets) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            data.back_up_key(dir, &m.name, &format!("secrets/{name}"))?;
        }
    }
    Ok(())
}

/// The module's Compose volumes removed, then its folder.
pub(crate) fn remove(paths: &Paths, postgres: &Postgres, module: &str) -> Result<(), String> {
    let label = format!("label=com.docker.compose.project={module}");
    let list = Cmd::new("docker", ["volume", "ls", "--quiet", "--filter", &label]);
    let volumes = postgres.runner.run(&list)?;
    for v in volumes.lines().map(str::trim).filter(|v| !v.is_empty()) {
        postgres
            .runner
            .run(&Cmd::new("docker", ["volume", "rm", v]))?;
    }
    let folder = paths.install_dir().join(module);
    match std::fs::remove_dir_all(&folder) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("{}: {e}", folder.display()))
        }
        _ => Ok(()),
    }
}
