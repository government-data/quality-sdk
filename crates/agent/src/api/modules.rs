//! The `updater/v1` calls of a module's lifecycle (decision 0009, point
//! 6): its OIDC clients, its purge, and the agent's audit events with
//! their outbox.
//!
//! | Call | Method |
//! | --- | --- |
//! | `PUT /updater/v1/modules/{module}/clients` | [`UpdaterApi::put_clients`] |
//! | `POST /updater/v1/modules/{module}/clients/disable` | [`UpdaterApi::disable_clients`] |
//! | `DELETE /updater/v1/modules/{module}` | [`UpdaterApi::delete_module`] |
//! | `POST /updater/v1/audit` | [`UpdaterApi::audit`], [`Outbox`] |
//!
//! A client's secret travels in a request body only, over mTLS; no body is
//! logged and an error carries the core's `error` text only.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use module_manifest::{RoleClaim, TokenAlg};
use serde::{Deserialize, Serialize};

use super::UpdaterApi;
use crate::module_secrets::Secret;

/// The body of `PUT …/clients`: the module's clients, each with its
/// secret; a client it does not name is disabled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Clients {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub people: Option<PeopleClient>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<MachineClient>,
}

/// The people's client as the manifest declares it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeopleClient {
    pub redirect_paths: Vec<String>,
    pub logout_paths: Vec<String>,
    pub alg: TokenAlg,
    pub pkce: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_claim: Option<RoleClaim>,
    pub secret: Secret,
    /// The previous secret is accepted too, for at most one hour (a
    /// rotation).
    pub keep_previous: bool,
}

/// The machine client: the capabilities it consumes, its audiences and
/// the capabilities the module requires (by which the core grants some
/// audiences; left out when empty, for a core that predates it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MachineClient {
    pub capabilities: Vec<String>,
    pub audiences: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    pub secret: Secret,
    pub keep_previous: bool,
}

/// What the core answers to `PUT …/clients`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ClientsAnswer {
    pub issuer: String,
    #[serde(default)]
    pub people_client_id: Option<String>,
    #[serde(default)]
    pub machine_client_id: Option<String>,
}

/// An agent's act, recorded in the core's audit chain with the agent's
/// machine as the actor: no secret, no person, no host user, no data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// `ModuleAdded`, `ModuleRemoved`, `ModulePurgeStarted`,
    /// `ModulePurged`, `ModuleSecretsRotated`.
    pub event: String,
    pub module: String,
    pub version: String,
    /// `succeeded` or `failed`.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A module name as a path segment.
fn segment(module: &str) -> Result<&str, String> {
    let ok = !module.is_empty()
        && module
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(module)
    } else {
        Err(format!("{module:?} cannot name a module in a call"))
    }
}

impl UpdaterApi {
    /// `PUT /updater/v1/modules/{module}/clients`: register or update the
    /// module's clients and enable them. Sending the same secret again
    /// changes nothing.
    pub fn put_clients(&self, module: &str, clients: &Clients) -> Result<ClientsAnswer, String> {
        let path = format!("/modules/{}/clients", segment(module)?);
        let body = serde_json::to_value(clients).map_err(|e| e.to_string())?;
        let answer = self.call("PUT", &path, Some(&body))?;
        serde_json::from_value(answer)
            .map_err(|e| format!("PUT {}{path}: its answer does not read: {e}", super::PREFIX))
    }

    /// `POST /updater/v1/modules/{module}/clients/disable`: no code, no
    /// token, refresh tokens revoked.
    pub fn disable_clients(&self, module: &str) -> Result<(), String> {
        let path = format!("/modules/{}/clients/disable", segment(module)?);
        self.call("POST", &path, Some(&serde_json::json!({})))
            .map(|_| ())
    }

    /// `DELETE /updater/v1/modules/{module}`: the purge of its clients,
    /// managed policies, their assignments and its actions.
    pub fn delete_module(&self, module: &str) -> Result<(), String> {
        let path = format!("/modules/{}", segment(module)?);
        self.call("DELETE", &path, None).map(|_| ())
    }

    /// `POST /updater/v1/audit`: one event.
    pub fn audit(&self, event: &AuditEvent) -> Result<(), String> {
        let body = serde_json::to_value(event).map_err(|e| e.to_string())?;
        self.call("POST", "/audit", Some(&body)).map(|_| ())
    }
}

/// The events the core could not take, one JSON line each
/// (`/var/lib/quality-data/updater/audit-outbox.jsonl`, `0600`), sent in
/// order when the service starts.
#[derive(Debug, Clone)]
pub struct Outbox {
    pub path: PathBuf,
}

impl Outbox {
    pub fn new(paths: &crate::paths::Paths) -> Self {
        Self {
            path: paths.audit_outbox(),
        }
    }

    /// Send `event`; when the core does not take it, keep it for later
    /// (a warning, not a failure).
    pub fn send(&self, api: &UpdaterApi, event: &AuditEvent) -> Result<(), String> {
        let Err(err) = api.audit(event) else {
            return Ok(());
        };
        tracing::warn!(
            event = %event.event,
            module = %event.module,
            "the audit event waits in the outbox: {err}"
        );
        self.keep(event)
    }

    /// Keep `event` for later, without trying to send it.
    pub fn keep(&self, event: &AuditEvent) -> Result<(), String> {
        let line = serde_json::to_string(event).map_err(|e| e.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        writeln!(f, "{line}")
            .and_then(|()| f.sync_all())
            .map_err(|e| format!("{}: {e}", self.path.display()))
    }

    /// Every waiting event sent in order, up to the first the core does
    /// not take; those left stay. How many were sent.
    pub fn flush(&self, api: &UpdaterApi) -> Result<usize, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut sent = 0;
        for line in &lines {
            let event: AuditEvent = serde_json::from_str(line)
                .map_err(|e| format!("{}: a line does not read: {e}", self.path.display()))?;
            if let Err(err) = api.audit(&event) {
                tracing::debug!("the audit outbox waits: {err}");
                break;
            }
            sent += 1;
        }
        if sent == lines.len() {
            std::fs::remove_file(&self.path)
                .map_err(|e| format!("{}: {e}", self.path.display()))?;
        } else if sent > 0 {
            let rest: String = lines[sent..].iter().map(|l| format!("{l}\n")).collect();
            let tmp = self.path.with_extension("partial");
            let _ = std::fs::remove_file(&tmp);
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| format!("{}: {e}", tmp.display()))?;
            f.write_all(rest.as_bytes())
                .and_then(|()| f.sync_all())
                .and_then(|()| std::fs::rename(&tmp, &self.path))
                .map_err(|e| format!("{}: {e}", self.path.display()))?;
        }
        Ok(sent)
    }
}

#[cfg(test)]
#[path = "modules_tests.rs"]
mod tests;
