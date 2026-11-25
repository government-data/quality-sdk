//! What a module gets at `add` and at an update (decision 0009, point 7,
//! steps 4 to 6): its secret files, its role and databases, its OIDC
//! clients and the `QD_*` lines of its `.env`; and what `remove` takes
//! away (point 8): its clients disabled and its role without `LOGIN`.
//! Each step is idempotent: run twice, it changes nothing.

use std::path::PathBuf;

use module_manifest::{
    Manifest, SecretContent, DATABASE_PASSWORD, OIDC_MACHINE_SECRET, OIDC_PEOPLE_SECRET,
    SECRET_BYTES,
};
use serde::{Deserialize, Serialize};

use crate::api::modules::{AuditEvent, Clients, ClientsAnswer, MachineClient, PeopleClient};
use crate::host::Features;
use crate::module_secrets::Secret;

/// The files the agent issues: random bytes of the default size.
const ISSUED: SecretContent = SecretContent::Random {
    bytes: SECRET_BYTES,
};

/// The port of the core's PostgreSQL on `data-quality-db`.
pub const DATABASE_PORT: u16 = 5432;

/// What a module's provisioning acts on, beside [`super::ModuleHost`].
pub trait Provision {
    /// The core's `features` (it must answer).
    fn features(&self) -> Result<Features, String>;
    /// The module's progress file, while an operation runs.
    fn progress(&self, module: &str) -> Result<Option<Progress>, String>;
    fn save_progress(&self, module: &str, progress: &Progress) -> Result<(), String>;
    fn clear_progress(&self, module: &str) -> Result<(), String>;
    /// `/opt/quality-data/<module>/secrets`.
    fn secrets_dir(&self, module: &str) -> PathBuf;
    /// The secret file `name`, made unless it exists, owned by `uid`.
    fn ensure_secret(
        &self,
        module: &str,
        name: &str,
        content: SecretContent,
        uid: u32,
    ) -> Result<(), String>;
    fn secret(&self, module: &str, name: &str) -> Result<Secret, String>;
    /// `<name>.next` with a new value, unless one is there (rotation).
    fn stage_next(&self, module: &str, name: &str, bytes: u32) -> Result<(), String>;
    fn read_next(&self, module: &str, name: &str) -> Result<Option<Secret>, String>;
    /// `<name>.next` renamed over `<name>`, owned by `uid`.
    fn promote_next(&self, module: &str, name: &str, uid: u32) -> Result<(), String>;
    /// The module's `.env` with the agent's `QD_*` lines.
    fn write_env(&self, module: &str, vars: &[(&str, String)]) -> Result<(), String>;
    /// The installation's CA copied to the module's folder: its path.
    fn copy_ca(&self, module: &str) -> Result<PathBuf, String>;
    /// The name the core's PostgreSQL is reached by (`POSTGRES_SERVER_NAME`).
    fn database_host(&self) -> Result<String, String>;
    fn create_database(
        &self,
        role: &str,
        password: &Secret,
        databases: &[String],
    ) -> Result<(), String>;
    fn database_login(&self, role: &str, login: bool) -> Result<(), String>;
    fn put_clients(&self, module: &str, clients: &Clients) -> Result<ClientsAnswer, String>;
    fn disable_clients(&self, module: &str) -> Result<(), String>;
    /// Record an event in the core's audit chain, or keep it for later.
    fn audit(&self, event: &AuditEvent);
    /// Record an event in the core's audit chain; the core must answer.
    fn audit_required(&self, event: &AuditEvent) -> Result<(), String>;
    /// A purge's backup of the module, checked, `complete` written last;
    /// its folder. A piece that is not critical and fails is a warning.
    fn purge_backup(&self, m: &Manifest, warnings: &mut Vec<String>) -> Result<PathBuf, String>;
    /// `DELETE /updater/v1/modules/{module}`.
    fn delete_module(&self, module: &str) -> Result<(), String>;
    /// The databases dropped with their sessions, then the role.
    fn drop_database(&self, role: &str, databases: &[String]) -> Result<(), String>;
    /// The module's Compose volumes, then its folder, removed.
    fn purge_files(&self, module: &str) -> Result<(), String>;
}

/// A module's operation while it runs, written before each step that
/// changes something (`/var/lib/quality-data/updater/modules/<module>.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// `adding`, `removing` (its clients left to disable), `purging` or
    /// `rotating`.
    pub operation: String,
    /// The module's version.
    pub version: String,
    /// The step reached.
    pub step: String,
}

/// Whether a manifest asks for anything the agent provides.
pub fn provides_anything(m: &Manifest) -> bool {
    m.oidc.is_some() || !m.secrets.generate.is_empty() || m.database_role().is_some()
}

/// The core's features a manifest needs, refused with the reason when
/// the core does not have them (point 6). A menu with `requires` needs
/// none: it is hidden from an older core.
pub fn check_features(m: &Manifest, features: &Features) -> Result<(), String> {
    let need = [
        (m.oidc.is_some(), Features::MODULE_CLIENTS, "OIDC clients"),
        (
            m.permissions.is_some(),
            Features::MODULE_PERMISSIONS,
            "permissions",
        ),
    ];
    for (uses, feature, what) in need {
        if uses && !features.has(feature) {
            return Err(format!(
                "{} declares {what}, which this quality-data does not take: it needs a \
                 quality-data whose updater/v1 has the feature {feature:?}",
                m.name
            ));
        }
    }
    Ok(())
}

/// The audiences of the machine client: each consumed capability's
/// `token-audience`, as its provider among `installed` declares it.
pub fn audiences(m: &Manifest, installed: &[Manifest]) -> Result<Vec<String>, String> {
    let Some(machine) = m.oidc.as_ref().and_then(|o| o.machine.as_ref()) else {
        return Ok(Vec::new());
    };
    machine
        .capabilities
        .iter()
        .map(|c| {
            installed
                .iter()
                .filter(|p| p.name != m.name)
                .flat_map(|p| &p.provides.capabilities)
                .filter(|p| &p.name == c)
                .find_map(|p| p.properties.get("token-audience")?.as_str())
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "{} consumes {c}, whose provider declares no token-audience",
                        m.name
                    )
                })
        })
        .collect()
}

/// Steps 4 to 6 of point 7 for `m`, each saved in `progress` before it
/// runs (`save`); `audiences` from [`audiences`].
pub fn provision(
    p: &dyn Provision,
    m: &Manifest,
    audiences: &[String],
    save: &mut dyn FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    let uid = m.secrets.uid;
    save("secrets")?;
    for g in &m.secrets.generate {
        p.ensure_secret(&m.name, &g.name, g.content(), uid)?;
    }
    let mut env: Vec<(&str, String)> = Vec::new();
    if provides_anything(m) {
        let dir = p.secrets_dir(&m.name);
        env.push(("QD_SECRETS_DIR", dir.display().to_string()));
    }
    if let Some(role) = m.database_role() {
        save("database")?;
        p.ensure_secret(&m.name, DATABASE_PASSWORD, ISSUED, uid)?;
        let password = p.secret(&m.name, DATABASE_PASSWORD)?;
        let dbs: Vec<String> = m.created_databases().map(|d| d.name.clone()).collect();
        p.create_database(&role, &password, &dbs)?;
        let ca = p.copy_ca(&m.name)?;
        env.push(("QD_DB_HOST", p.database_host()?));
        env.push(("QD_DB_PORT", DATABASE_PORT.to_string()));
        env.push(("QD_DB_USER", role.clone()));
        env.push(("QD_DB_NAME", dbs[0].clone()));
        env.push(("QD_DB_CA_FILE", ca.display().to_string()));
    }
    if let Some(oidc) = &m.oidc {
        save("clients")?;
        let clients = clients(p, m, oidc, audiences)?;
        let answer = p.put_clients(&m.name, &clients)?;
        env.push(("QD_OIDC_ISSUER", answer.issuer));
        if let Some(id) = answer.people_client_id {
            env.push(("QD_OIDC_PEOPLE_CLIENT_ID", id));
        }
        if let Some(id) = answer.machine_client_id {
            env.push(("QD_OIDC_MACHINE_CLIENT_ID", id));
        }
    }
    save("env")?;
    p.write_env(&m.name, &env)
}

/// The clients' body: each issued secret made only if absent.
fn clients(
    p: &dyn Provision,
    m: &Manifest,
    oidc: &module_manifest::Oidc,
    audiences: &[String],
) -> Result<Clients, String> {
    let uid = m.secrets.uid;
    if oidc.people.is_some() {
        p.ensure_secret(&m.name, OIDC_PEOPLE_SECRET, ISSUED, uid)?;
    }
    if oidc.machine.is_some() {
        p.ensure_secret(&m.name, OIDC_MACHINE_SECRET, ISSUED, uid)?;
    }
    clients_with(oidc, audiences, |name| p.secret(&m.name, name))
}

/// The clients' body with each secret from `value` (by its file's name),
/// not keeping the previous ones.
pub fn clients_with(
    oidc: &module_manifest::Oidc,
    audiences: &[String],
    value: impl Fn(&str) -> Result<Secret, String>,
) -> Result<Clients, String> {
    let mut clients = Clients::default();
    if let Some(people) = &oidc.people {
        clients.people = Some(PeopleClient {
            redirect_paths: people.redirect_paths.clone(),
            logout_paths: people.logout_paths.clone(),
            alg: people.alg,
            pkce: people.pkce,
            role_claim: people.role_claim.clone(),
            secret: value(OIDC_PEOPLE_SECRET)?,
            keep_previous: false,
        });
    }
    if let Some(machine) = &oidc.machine {
        clients.machine = Some(MachineClient {
            capabilities: machine.capabilities.clone(),
            audiences: audiences.to_vec(),
            secret: value(OIDC_MACHINE_SECRET)?,
            keep_previous: false,
        });
    }
    Ok(clients)
}

/// What `remove` takes away (point 8): the clients disabled, the role
/// without `LOGIN` and its sessions ended. Whether the clients are left to
/// disable: a core that does not answer leaves a warning, and the service
/// disables them once it answers.
pub fn deprovision(
    p: &dyn Provision,
    m: &Manifest,
    warnings: &mut Vec<String>,
) -> Result<bool, String> {
    let mut left = false;
    if m.oidc.is_some() {
        if let Err(err) = p.disable_clients(&m.name) {
            warnings.push(format!(
                "{}'s clients were not disabled ({err}); the service disables them once \
                 quality-data answers",
                m.name
            ));
            left = true;
        }
    }
    if let Some(role) = m.database_role() {
        p.database_login(&role, false)?;
    }
    Ok(left)
}

/// An audit event of the agent's own.
pub fn event(name: &str, m: &str, version: &str, failed: Option<(&str, &str)>) -> AuditEvent {
    AuditEvent {
        event: name.into(),
        module: m.into(),
        version: version.into(),
        outcome: if failed.is_some() {
            "failed"
        } else {
            "succeeded"
        }
        .into(),
        step: failed.map(|(s, _)| s.to_string()),
        error: failed.map(|(_, e)| e.to_string()),
    }
}
