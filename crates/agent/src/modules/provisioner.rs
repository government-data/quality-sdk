//! The [`Provision`] of an installation (decision 0009): the secrets
//! adapter, the PostgreSQL adapter, quality-data's `updater/v1` and the
//! module's progress file.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use super::provision::{Progress, Provision};
use crate::api::modules::{AuditEvent, Clients, ClientsAnswer, Outbox};
use crate::api::UpdaterApi;
use crate::host::{Api, Features};
use crate::module_secrets::{ModuleSecrets, Secret};
use crate::paths::Paths;
use crate::postgres::Postgres;

/// The installation's adapters for what a module gets.
pub struct Provisioner {
    paths: Paths,
    secrets: ModuleSecrets,
    postgres: Postgres,
    /// quality-data's `updater/v1` (none: the settings name no API).
    api: Option<UpdaterApi>,
    outbox: Outbox,
}

impl std::fmt::Debug for Provisioner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provisioner")
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

impl Provisioner {
    /// At `paths`, with `api` (none: every call of the core fails) and the
    /// agent's helper image for the PostgreSQL adapter.
    pub fn new(paths: &Paths, api: Option<UpdaterApi>, helper: &str) -> Self {
        Self {
            paths: paths.clone(),
            secrets: ModuleSecrets::new(paths),
            postgres: Postgres::new(paths, helper),
            api,
            outbox: Outbox::new(paths),
        }
    }

    fn api(&self) -> Result<&UpdaterApi, String> {
        self.api.as_ref().ok_or_else(|| {
            format!(
                "{}: no [api] url, quality-data's updater/v1",
                self.paths.config_file().display()
            )
        })
    }
}

impl Provision for Provisioner {
    fn features(&self) -> Result<Features, String> {
        self.api()?.features()
    }
    fn progress(&self, module: &str) -> Result<Option<Progress>, String> {
        let path = self.paths.module_progress(module);
        match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
    /// Written whole: a temporary file, synced, renamed.
    fn save_progress(&self, module: &str, progress: &Progress) -> Result<(), String> {
        let path = self.paths.module_progress(module);
        let tmp = path.with_extension("partial");
        let json = serde_json::to_vec_pretty(progress).map_err(|e| e.to_string())?;
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let _ = std::fs::remove_file(&tmp);
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &path)
        };
        write().map_err(|e| format!("{}: {e}", path.display()))
    }
    fn clear_progress(&self, module: &str) -> Result<(), String> {
        let path = self.paths.module_progress(module);
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("{}: {e}", path.display()))
            }
            _ => Ok(()),
        }
    }
    fn secrets_dir(&self, module: &str) -> PathBuf {
        self.secrets.dir(module)
    }
    fn ensure_secret(&self, module: &str, name: &str, bytes: u32, uid: u32) -> Result<(), String> {
        self.secrets.ensure(module, name, bytes, uid).map(|_| ())
    }
    fn secret(&self, module: &str, name: &str) -> Result<Secret, String> {
        self.secrets.read(module, name)
    }
    fn write_env(&self, module: &str, vars: &[(&str, String)]) -> Result<(), String> {
        self.secrets.write_env(module, vars)
    }
    fn copy_ca(&self, module: &str) -> Result<PathBuf, String> {
        self.secrets.copy_ca(module)
    }
    /// `POSTGRES_SERVER_NAME` of the core's `.env`: the alias PostgreSQL
    /// has on `data-quality-db`, and the name in its certificate.
    fn database_host(&self) -> Result<String, String> {
        let env = self.paths.install_dir().join("quality-data/.env");
        let text = std::fs::read_to_string(&env).map_err(|e| format!("{}: {e}", env.display()))?;
        text.lines()
            .filter_map(|l| l.trim().strip_prefix("POSTGRES_SERVER_NAME="))
            .map(|v| v.trim().trim_matches('"').to_string())
            .rfind(|v| !v.is_empty())
            .ok_or_else(|| format!("{}: no POSTGRES_SERVER_NAME", env.display()))
    }
    fn create_database(&self, role: &str, password: &Secret, dbs: &[String]) -> Result<(), String> {
        self.postgres.create_module(role, password, dbs)
    }
    fn database_login(&self, role: &str, login: bool) -> Result<(), String> {
        self.postgres.module_login(role, login)
    }
    fn put_clients(&self, module: &str, clients: &Clients) -> Result<ClientsAnswer, String> {
        self.api()?.put_clients(module, clients)
    }
    fn disable_clients(&self, module: &str) -> Result<(), String> {
        self.api()?.disable_clients(module)
    }
    /// Sent, or kept in the outbox (a warning, never a failure).
    fn audit(&self, event: &AuditEvent) {
        let kept = match self.api() {
            Ok(api) => self.outbox.send(api, event),
            Err(_) => self.outbox.keep(event),
        };
        if let Err(err) = kept {
            tracing::warn!(event = %event.event, "the audit event was lost: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provisioner(name: &str) -> (Provisioner, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("agent-provisioner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (Provisioner::new(&Paths::under(&root), None, "unused"), root)
    }

    #[test]
    fn the_progress_file_is_written_whole_read_and_cleared() {
        let (p, root) = provisioner("progress");
        assert_eq!(p.progress("bi").unwrap(), None);
        let progress = Progress {
            operation: "adding".into(),
            version: "1.0.0".into(),
            step: "clients".into(),
        };
        p.save_progress("bi", &progress).unwrap();
        assert_eq!(p.progress("bi").unwrap(), Some(progress));
        let file = root.join("var/lib/quality-data/updater/modules/bi.json");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        p.clear_progress("bi").unwrap();
        p.clear_progress("bi").unwrap();
        assert_eq!(p.progress("bi").unwrap(), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_database_host_is_the_cores_server_name_and_without_an_api_the_core_fails() {
        let (p, root) = provisioner("host");
        let env = root.join("opt/quality-data/quality-data/.env");
        std::fs::create_dir_all(env.parent().unwrap()).unwrap();
        std::fs::write(
            &env,
            "POSTGRES_DB=quality_data\nPOSTGRES_SERVER_NAME=app.qs-test.internal\n",
        )
        .unwrap();
        assert_eq!(p.database_host().unwrap(), "app.qs-test.internal");
        std::fs::write(&env, "POSTGRES_SERVER_NAME=\n").unwrap();
        assert!(p
            .database_host()
            .unwrap_err()
            .contains("no POSTGRES_SERVER_NAME"));
        assert!(p.features().unwrap_err().contains("no [api] url"));
        // Without an API an audit event waits in the outbox.
        p.audit(&super::super::provision::event(
            "ModuleAdded",
            "bi",
            "1.0.0",
            None,
        ));
        assert!(root
            .join("var/lib/quality-data/updater/audit-outbox.jsonl")
            .exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
