//! `qd module rotate <module>` (decision 0009, point 4): new values of the
//! files the agent issues (`oidc-people-secret`, `oidc-machine-secret`,
//! `database-password`); the module's own `secrets.generate` files are
//! not rotated. Each step is written to the module's progress file
//! (`rotating`) before it runs; a crash resumes at the last step
//! recorded, and a `.next` left behind is reused, never replaced.
//!
//! 1. `stage`: each new value written aside as `<name>.next`.
//! 2. `clients`: the core told the new secrets with `keep_previous`.
//! 3. `database`: the role's password set from the new value.
//! 4. `promote`: each `.next` renamed over its file, the module recreated
//!    and its health waited for.
//! 5. `finish`: the core told the same secrets without `keep_previous`.

use module_manifest::{
    Manifest, DATABASE_PASSWORD, OIDC_MACHINE_SECRET, OIDC_PEOPLE_SECRET, SECRET_BYTES,
};

use super::provision::{self, Progress, Provision};
use super::{ModuleHost, Warnings};

/// The steps, in order.
const STEPS: [&str; 5] = ["stage", "clients", "database", "promote", "finish"];

/// The files the agent issues for `m`.
fn issued(m: &Manifest) -> Vec<&'static str> {
    let oidc = m.oidc.as_ref();
    let mut names = Vec::new();
    if oidc.is_some_and(|o| o.people.is_some()) {
        names.push(OIDC_PEOPLE_SECRET);
    }
    if oidc.is_some_and(|o| o.machine.is_some()) {
        names.push(OIDC_MACHINE_SECRET);
    }
    if m.database_role().is_some() {
        names.push(DATABASE_PASSWORD);
    }
    names
}

/// The new value of `name`: its `.next`, or the file once promoted.
fn new_value(
    p: &dyn Provision,
    module: &str,
    name: &str,
) -> Result<crate::module_secrets::Secret, String> {
    match p.read_next(module, name)? {
        Some(next) => Ok(next),
        None => p.secret(module, name),
    }
}

/// Rotate the issued secrets of the installed `module`.
pub fn rotate(host: &dyn ModuleHost, module: &str) -> Result<Warnings, String> {
    host.no_update_running()?;
    let p = host.provision();
    let (installed, _) = host.installed()?;
    if !installed.modules.contains_key(module) {
        return Err(format!("{module} is not installed"));
    }
    let resume_at = match p.progress(module)? {
        Some(pr) if pr.operation == "rotating" => {
            STEPS.iter().position(|s| *s == pr.step).unwrap_or(0)
        }
        Some(pr) => {
            return Err(format!(
                "{module} is {} ({}): finish that first",
                pr.operation, pr.step
            ))
        }
        None => 0,
    };
    let m = host
        .manifest(module)?
        .ok_or_else(|| format!("{module} has no module.yaml"))?;
    let names = issued(&m);
    if names.is_empty() {
        return Err(format!("{module} has no secret the agent issues"));
    }
    if m.oidc.is_some() {
        provision::check_features(&m, &p.features()?)?;
    }
    let version = m.version.to_string();
    let mut manifests = Vec::new();
    for name in installed.modules.keys() {
        manifests.extend(host.manifest(name)?);
    }
    let audiences = provision::audiences(&m, &manifests)?;
    let mut at = String::new();
    let rotated = (|| {
        for (i, step) in STEPS.iter().enumerate().skip(resume_at) {
            at = (*step).to_string();
            p.save_progress(
                module,
                &Progress {
                    operation: "rotating".into(),
                    version: version.clone(),
                    step: at.clone(),
                },
            )?;
            run_step(host, &m, &names, &audiences, i)?;
        }
        Ok::<(), String>(())
    })();
    let failed = rotated.as_ref().err().map(|e| (at.as_str(), e.as_str()));
    p.audit(&provision::event(
        "ModuleSecretsRotated",
        module,
        &version,
        failed,
    ));
    match &rotated {
        Ok(()) => host.report("module_rotated", &installed.version, module, &version, ""),
        Err(err) => host.report("rotate_failed", &installed.version, module, &version, err),
    }
    rotated.map_err(|e| {
        format!("{e} ({module} stopped at {at}: qd module rotate {module} resumes it)")
    })?;
    p.clear_progress(module)?;
    Ok(Warnings::new())
}

fn run_step(
    host: &dyn ModuleHost,
    m: &Manifest,
    names: &[&str],
    audiences: &[String],
    step: usize,
) -> Result<(), String> {
    let p = host.provision();
    let module = m.name.as_str();
    match STEPS[step] {
        "stage" => names
            .iter()
            .try_for_each(|n| p.stage_next(module, n, SECRET_BYTES)),
        "clients" => tell_clients(p, m, audiences, true),
        "database" => match m.database_role() {
            Some(role) => {
                let dbs: Vec<String> = m.created_databases().map(|d| d.name.clone()).collect();
                p.create_database(&role, &new_value(p, module, DATABASE_PASSWORD)?, &dbs)
            }
            None => Ok(()),
        },
        "promote" => {
            for n in names {
                p.promote_next(module, n, m.secrets.uid)?;
            }
            host.up(module)?;
            host.healthy(module)
        }
        _ => tell_clients(p, m, audiences, false),
    }
}

/// `PUT …/clients` with the new secrets, keeping the previous ones or not.
fn tell_clients(
    p: &dyn Provision,
    m: &Manifest,
    audiences: &[String],
    keep_previous: bool,
) -> Result<(), String> {
    let Some(oidc) = &m.oidc else { return Ok(()) };
    let mut clients = provision::clients_with(oidc, audiences, |name| new_value(p, &m.name, name))?;
    if let Some(c) = clients.people.as_mut() {
        c.keep_previous = keep_previous;
    }
    if let Some(c) = clients.machine.as_mut() {
        c.keep_previous = keep_previous;
    }
    p.put_clients(&m.name, &clients).map(|_| ())
}

#[cfg(test)]
#[path = "rotate_tests.rs"]
mod tests;
