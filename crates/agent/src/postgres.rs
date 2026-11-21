//! The PostgreSQL adapter (decision 0003): back up every database before an
//! update and check the backup, and restore it when going back failed, as
//! `tools/verify/postgres-backup.sh` verified on the PostgreSQL shipped.
//!
//! Every program runs inside the database's own container (`docker compose
//! exec -T`), so `pg_dump` is the server's version and the credentials are
//! the container's (`$POSTGRES_USER`); the agent holds none. A backup is a
//! directory `/var/lib/quality-data/backups/<time>/` (`0700`):
//!
//! - `globals.sql`: roles, from `pg_dumpall --globals-only` (kept for a
//!   person; a restore into the same server needs none);
//! - `<database>.dump`: each database, `pg_dump --format=custom`;
//! - `databases`: their names, written last, once every dump read back
//!   whole (`pg_restore --list` and a full read: a dump cut short passes
//!   `--list` alone). Without it there is no backup.
//!
//! Before `databases` is written, the critical data the installed modules'
//! manifests name (their volumes and keys) is backed up beside the dumps
//! and checked ([`ModuleData`]); a restore puts it back after the
//! databases.

use std::path::{Path, PathBuf};

use crate::command::{Cmd, Runner};
use crate::compose;
use crate::module_data::ModuleData;

/// How many backups are kept.
pub const KEEP: usize = 10;

/// The databases of the server, but the maintenance one.
const DATABASES: &str = "select datname from pg_database \
    where not datistemplate and datname <> 'postgres' order by 1";

/// The PostgreSQL adapter.
pub struct Postgres {
    /// `/opt/quality-data`.
    pub install: PathBuf,
    /// The module and the Compose service that run the database.
    pub module: String,
    pub service: String,
    /// `/var/lib/quality-data/backups`.
    pub backups: PathBuf,
    pub runner: Box<dyn Runner>,
    /// How long the database's service may take to be healthy.
    pub wait_secs: u64,
    /// The modules' volumes and keys.
    pub data: ModuleData,
}

impl std::fmt::Debug for Postgres {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Postgres")
            .field("module", &self.module)
            .field("service", &self.service)
            .field("backups", &self.backups)
            .finish_non_exhaustive()
    }
}

/// A database name the agent passes on: letters, digits, `_` and `-`.
fn check_database(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!("the database {name:?} cannot name a backup file"))
    }
}

impl Postgres {
    /// quality-data's `postgres` service, at `paths`.
    pub fn new(paths: &crate::paths::Paths) -> Self {
        Self {
            install: paths.install_dir().to_path_buf(),
            module: "quality-data".into(),
            service: "postgres".into(),
            backups: paths.backups_dir(),
            runner: Box::new(crate::command::System),
            wait_secs: 300,
            data: ModuleData::new(paths),
        }
    }

    /// `script` run by `sh` in the database's container as its superuser;
    /// `args` are `$1`... (never part of the script).
    fn in_database(&self, script: &str, args: &[&str]) -> Result<Cmd, String> {
        let mut all = vec!["exec", "-T", &self.service, "sh", "-c", script, "sh"];
        all.extend_from_slice(args);
        compose::command(&self.install, &self.module, &all)
    }

    fn run(&self, cmd: &Cmd) -> Result<String, String> {
        self.runner.run(cmd)
    }

    /// The databases to back up.
    fn databases(&self) -> Result<Vec<String>, String> {
        let script = format!(
            r#"psql -U "$POSTGRES_USER" -d postgres -v ON_ERROR_STOP=1 -Atc "{DATABASES}""#
        );
        let out = self.run(&self.in_database(&script, &[])?)?;
        let names: Vec<String> = out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        for n in &names {
            check_database(n)?;
        }
        if names.is_empty() {
            return Err("the server has no database to back up".into());
        }
        Ok(names)
    }

    /// The dump at `file` reads back whole and holds something.
    fn check_dump(&self, file: &Path) -> Result<(), String> {
        let list = self.run(&self.in_database("pg_restore --list", &[])?.stdin(file))?;
        let entries = list
            .lines()
            .filter(|l| !l.starts_with(';') && !l.trim().is_empty())
            .count();
        if entries == 0 {
            return Err(format!("{} holds nothing", file.display()));
        }
        self.run(
            &self
                .in_database("pg_restore --file=/dev/null", &[])?
                .stdin(file),
        )
        .map_err(|e| format!("{} does not read back whole: {e}", file.display()))?;
        Ok(())
    }

    /// Every backup but the newest [`KEEP`] removed (only directories this
    /// adapter names).
    fn prune(&self) -> Result<(), String> {
        let mut all: Vec<PathBuf> = std::fs::read_dir(&self.backups)
            .map_err(|e| format!("{}: {e}", self.backups.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_backup_name)
            })
            .collect();
        all.sort();
        for old in all.iter().take(all.len().saturating_sub(KEEP)) {
            std::fs::remove_dir_all(old).map_err(|e| format!("{}: {e}", old.display()))?;
        }
        Ok(())
    }
}

/// How backups are named: the time, to the millisecond, so they sort.
const NAME: &str = "%Y%m%dT%H%M%S%3fZ";

fn is_backup_name(name: &str) -> bool {
    chrono::NaiveDateTime::parse_from_str(name, NAME).is_ok()
}

impl Postgres {
    /// Every database dumped into `dir` and read back whole, the modules'
    /// critical data beside them, then the databases' names written: how
    /// many databases.
    fn dump_into(&self, dir: &Path) -> Result<usize, String> {
        let databases = self.databases()?;
        let globals = r#"pg_dumpall -U "$POSTGRES_USER" --globals-only"#;
        self.run(
            &self
                .in_database(globals, &[])?
                .stdout(&dir.join("globals.sql")),
        )?;
        for db in &databases {
            let file = dir.join(format!("{db}.dump"));
            let dump = r#"pg_dump -U "$POSTGRES_USER" -d "$1" --format=custom"#;
            self.run(&self.in_database(dump, &[db])?.stdout(&file))?;
            self.check_dump(&file)?;
        }
        let pieces = self.data.back_up_into(dir, &databases)?;
        tracing::info!(pieces, "module data backed up");
        let list = format!("{}\n", databases.join("\n"));
        std::fs::write(dir.join("databases"), list)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(databases.len())
    }
}

impl crate::host::Database for Postgres {
    /// Every database dumped and read back whole, or no backup (and no
    /// update).
    fn back_up(&self) -> Result<PathBuf, String> {
        use std::os::unix::fs::DirBuilderExt;
        let name = chrono::Utc::now().format(NAME).to_string();
        let dir = self.backups.join(&name);
        std::fs::create_dir_all(&self.backups)
            .map_err(|e| format!("{}: {e}", self.backups.display()))?;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        let dumped = self.dump_into(&dir);
        if dumped.is_err() {
            // Not a backup: nothing of it is kept.
            let _ = std::fs::remove_dir_all(&dir);
        }
        let databases = dumped?;
        self.prune()?;
        tracing::info!(backup = %dir.display(), databases, "backup checked");
        Ok(dir)
    }

    /// The database's service brought up alone (the host stopped every
    /// module), then each database restored in one transaction, then the
    /// modules' data.
    fn restore(&self, backup: &Path) -> Result<(), String> {
        let list = std::fs::read_to_string(backup.join("databases"))
            .map_err(|e| format!("{} is not a whole backup: {e}", backup.display()))?;
        let databases: Vec<&str> = list.lines().filter(|l| !l.is_empty()).collect();
        for db in &databases {
            check_database(db)?;
            let file = backup.join(format!("{db}.dump"));
            if !file.is_file() {
                return Err(format!("{} is missing", file.display()));
            }
        }
        let wait = self.wait_secs.to_string();
        let up = [
            "up",
            "--detach",
            "--no-build",
            "--wait",
            "--wait-timeout",
            &wait,
            &self.service,
        ];
        self.run(&compose::command(&self.install, &self.module, &up)?)?;
        for db in &databases {
            let restore = r#"pg_restore -U "$POSTGRES_USER" -d "$1" --clean --if-exists --single-transaction"#;
            let file = backup.join(format!("{db}.dump"));
            self.run(&self.in_database(restore, &[db])?.stdin(&file))
                .map_err(|e| format!("restoring {db}: {e}"))?;
        }
        tracing::warn!(backup = %backup.display(), "database restored");
        self.data.restore(backup)
    }
}

#[cfg(test)]
#[path = "postgres_tests.rs"]
mod tests;
