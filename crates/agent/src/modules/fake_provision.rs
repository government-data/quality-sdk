//! A [`Provision`] for tests: secrets, progress and `.env` in memory, the
//! core and PostgreSQL as notes, each able to fail or crash at a step.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::provision::{Progress, Provision};
use crate::api::modules::{AuditEvent, Clients, ClientsAnswer};
use crate::host::Features;
use crate::module_secrets::Secret;
use module_manifest::{Manifest, SecretContent};

#[derive(Debug, Default)]
pub(crate) struct FakeProvision {
    pub(crate) features: RefCell<Option<Features>>,
    pub(crate) progress: RefCell<BTreeMap<String, Progress>>,
    pub(crate) secrets: RefCell<BTreeMap<(String, String), String>>,
    pub(crate) env: RefCell<BTreeMap<String, Vec<(String, String)>>>,
    /// What was asked of the core and PostgreSQL, in order.
    pub(crate) notes: RefCell<Vec<String>>,
    pub(crate) events: RefCell<Vec<AuditEvent>>,
    /// Every body of `PUT …/clients`.
    pub(crate) sent: RefCell<Vec<Clients>>,
    /// The core does not answer `…/clients` and `…/disable`.
    pub(crate) core_down: RefCell<bool>,
    /// The purge's backup fails (a critical piece).
    pub(crate) backup_fails: RefCell<bool>,
    /// What `GET /updater/v1/removal/{module}` answers.
    pub(crate) refusals: RefCell<Vec<crate::api::modules::RemovalRefusal>>,
    /// Saving this step's progress fails, as a crash right before it.
    pub(crate) crash_at: RefCell<Option<String>>,
    counter: RefCell<u32>,
}

impl FakeProvision {
    pub(crate) fn with_features(names: &[&str]) -> Self {
        let f = Self::default();
        *f.features.borrow_mut() = Some(Features(names.iter().map(|s| s.to_string()).collect()));
        f
    }
    fn note(&self, s: String) {
        self.notes.borrow_mut().push(s);
    }
    pub(crate) fn notes(&self) -> Vec<String> {
        self.notes.borrow().clone()
    }
    pub(crate) fn secret_of(&self, module: &str, name: &str) -> Option<String> {
        self.secrets
            .borrow()
            .get(&(module.to_string(), name.to_string()))
            .cloned()
    }
    pub(crate) fn event_names(&self) -> Vec<String> {
        self.events
            .borrow()
            .iter()
            .map(|e| format!("{} {}", e.event, e.outcome))
            .collect()
    }
}

impl Provision for FakeProvision {
    fn features(&self) -> Result<Features, String> {
        self.features
            .borrow()
            .clone()
            .ok_or_else(|| "quality-data does not answer".to_string())
    }
    fn progress(&self, module: &str) -> Result<Option<Progress>, String> {
        Ok(self.progress.borrow().get(module).cloned())
    }
    fn save_progress(&self, module: &str, progress: &Progress) -> Result<(), String> {
        if self.crash_at.borrow().as_deref() == Some(progress.step.as_str()) {
            *self.crash_at.borrow_mut() = None;
            return Err(format!("crash before {}", progress.step));
        }
        self.progress
            .borrow_mut()
            .insert(module.into(), progress.clone());
        Ok(())
    }
    fn clear_progress(&self, module: &str) -> Result<(), String> {
        self.progress.borrow_mut().remove(module);
        Ok(())
    }
    fn secrets_dir(&self, module: &str) -> PathBuf {
        PathBuf::from(format!("/opt/quality-data/{module}/secrets"))
    }
    fn ensure_secret(
        &self,
        module: &str,
        name: &str,
        content: SecretContent,
        _uid: u32,
    ) -> Result<(), String> {
        let key = (module.to_string(), name.to_string());
        if !self.secrets.borrow().contains_key(&key) {
            *self.counter.borrow_mut() += 1;
            let value = format!("fake-{name}-{content:?}-{}", self.counter.borrow());
            self.secrets.borrow_mut().insert(key, value);
        }
        Ok(())
    }
    fn secret(&self, module: &str, name: &str) -> Result<Secret, String> {
        self.secret_of(module, name)
            .map(Secret::new)
            .ok_or_else(|| format!("{module}/secrets/{name} (read): not found"))
    }
    fn stage_next(&self, module: &str, name: &str, bytes: u32) -> Result<(), String> {
        let next = format!("{name}.next");
        self.ensure_secret(module, &next, SecretContent::Random { bytes }, 0)
    }
    fn read_next(&self, module: &str, name: &str) -> Result<Option<Secret>, String> {
        Ok(self
            .secret_of(module, &format!("{name}.next"))
            .map(Secret::new))
    }
    fn promote_next(&self, module: &str, name: &str, _uid: u32) -> Result<(), String> {
        let next = (module.to_string(), format!("{name}.next"));
        let mut secrets = self.secrets.borrow_mut();
        if let Some(v) = secrets.remove(&next) {
            secrets.insert((module.to_string(), name.to_string()), v);
        }
        self.note(format!("promote_next {name}"));
        Ok(())
    }
    fn write_env(&self, module: &str, vars: &[(&str, String)]) -> Result<(), String> {
        let vars = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        self.env.borrow_mut().insert(module.into(), vars);
        Ok(())
    }
    fn copy_ca(&self, module: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(format!("/opt/quality-data/{module}/ca.crt")))
    }
    fn database_host(&self) -> Result<String, String> {
        Ok("app.qs-test.internal".into())
    }
    fn create_database(&self, role: &str, password: &Secret, dbs: &[String]) -> Result<(), String> {
        self.note(format!(
            "create_database {role} {} {}",
            password.expose(),
            dbs.join(",")
        ));
        Ok(())
    }
    fn database_login(&self, role: &str, login: bool) -> Result<(), String> {
        self.note(format!("database_login {role} {login}"));
        Ok(())
    }
    fn put_clients(&self, module: &str, clients: &Clients) -> Result<ClientsAnswer, String> {
        if *self.core_down.borrow() {
            return Err("PUT /updater/v1/modules/x/clients: quality-data does not answer".into());
        }
        self.sent.borrow_mut().push(clients.clone());
        let people = clients
            .people
            .as_ref()
            .map(|c| c.secret.expose().to_string());
        let machine = clients
            .machine
            .as_ref()
            .map(|c| c.secret.expose().to_string());
        self.note(format!("put_clients {module} {people:?} {machine:?}"));
        Ok(ClientsAnswer {
            issuer: "https://qd.example/auth".into(),
            people_client_id: people.map(|_| module.to_string()),
            machine_client_id: machine.map(|_| format!("{module}-machine")),
        })
    }
    fn disable_clients(&self, module: &str) -> Result<(), String> {
        if *self.core_down.borrow() {
            return Err("quality-data does not answer".into());
        }
        self.note(format!("disable_clients {module}"));
        Ok(())
    }
    fn audit(&self, event: &AuditEvent) {
        self.events.borrow_mut().push(event.clone());
    }
    fn audit_required(&self, event: &AuditEvent) -> Result<(), String> {
        if *self.core_down.borrow() {
            return Err("quality-data does not answer".into());
        }
        self.audit(event);
        Ok(())
    }
    fn purge_backup(&self, m: &Manifest, _w: &mut Vec<String>) -> Result<PathBuf, String> {
        if *self.backup_fails.borrow() {
            return Err("the volume data cannot be read".into());
        }
        self.note(format!("purge_backup {}", m.name));
        Ok(PathBuf::from(format!(
            "/var/lib/quality-data/backups/purge/{}-t",
            m.name
        )))
    }
    fn delete_module(&self, module: &str) -> Result<(), String> {
        if *self.core_down.borrow() {
            return Err("quality-data does not answer".into());
        }
        self.note(format!("delete_module {module}"));
        Ok(())
    }
    fn removal_refusals(
        &self,
        module: &str,
    ) -> Result<Vec<crate::api::modules::RemovalRefusal>, String> {
        if *self.core_down.borrow() {
            return Err("quality-data does not answer".into());
        }
        self.note(format!("removal {module}"));
        Ok(self.refusals.borrow().clone())
    }
    fn drop_database(&self, role: &str, dbs: &[String]) -> Result<(), String> {
        self.note(format!("drop_database {role} {}", dbs.join(",")));
        Ok(())
    }
    fn purge_files(&self, module: &str) -> Result<(), String> {
        self.note(format!("purge_files {module}"));
        Ok(())
    }
}
