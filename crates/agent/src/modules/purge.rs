//! `qd module purge <module>` (decision 0009, point 8): everything of a
//! module deleted, after a backup, with a written confirmation (the
//! module's name). An installed module is removed first, with `remove`'s
//! refusal. Each step is written to the module's progress file
//! (`purging`) before it runs; a crash resumes at the last step recorded,
//! and `add` is refused while the file exists.
//!
//! 1. `started`: audit `ModulePurgeStarted`; the core must answer.
//! 2. `backup`: its databases, volumes, keys and whole secrets folder into
//!    `/var/lib/quality-data/backups/purge/<module>-<time>/`, `complete`
//!    written last; if a critical piece cannot be backed up, nothing is
//!    deleted.
//! 3. `delete`: the core told the installed modules (it refuses to purge
//!    one it still lists), then `DELETE /updater/v1/modules/{module}`.
//! 4. `drop`: its databases, then its role.
//! 5. `files`: its Compose volumes, then `/opt/quality-data/<module>/`.
//! 6. `done`: audit `ModulePurged`, the progress file removed.

use module_manifest::Manifest;

use super::provision::{self, Progress};
use super::{remove, ModuleHost, Warnings, CORE};

/// The steps, in order.
pub(crate) const STEPS: [&str; 6] = ["started", "backup", "delete", "drop", "files", "done"];

/// Purge `module`; `confirm` must be its name.
pub fn purge(host: &dyn ModuleHost, module: &str, confirm: &str) -> Result<Warnings, String> {
    if confirm != module {
        return Err(format!(
            "not purged: confirm with the module's name ({module}), which deletes its data"
        ));
    }
    let plain = !module.is_empty()
        && module
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !plain {
        return Err(format!("{module:?} cannot name a module"));
    }
    if module == CORE {
        return Err(format!("{CORE} is the core: it cannot be purged"));
    }
    host.no_update_running()?;
    let p = host.provision();
    let mut warnings = Warnings::new();
    let progress = p.progress(module)?;
    let resume_at = match &progress {
        Some(pr) if pr.operation == "purging" => {
            STEPS.iter().position(|s| *s == pr.step).unwrap_or(0)
        }
        _ => 0,
    };
    if resume_at == 0 {
        let (installed, _) = host.installed()?;
        let adding = progress.as_ref().is_some_and(|pr| pr.operation == "adding");
        if installed.modules.contains_key(module) || adding {
            warnings.extend(remove(host, module)?);
        } else {
            let manifests = super::manifests(host, installed.modules.keys())?;
            let m = host.manifest(module)?;
            super::removal::check(host, module, m.as_ref(), &manifests)?;
        }
    }
    let manifest = host.manifest(module)?;
    let version = match (&manifest, &progress) {
        (Some(m), _) => m.version.to_string(),
        (None, Some(pr)) if pr.operation == "purging" => pr.version.clone(),
        _ => return Err(format!("{module} has nothing to purge")),
    };
    for (i, step) in STEPS.iter().enumerate().skip(resume_at) {
        p.save_progress(
            module,
            &Progress {
                operation: "purging".into(),
                version: version.clone(),
                step: (*step).into(),
            },
        )?;
        run_step(host, module, &version, manifest.as_ref(), i, &mut warnings).map_err(|e| {
            format!("{e} ({module} stopped at {step}: qd module purge {module} resumes it)")
        })?;
    }
    p.clear_progress(module)?;
    Ok(warnings)
}

fn run_step(
    host: &dyn ModuleHost,
    module: &str,
    version: &str,
    m: Option<&Manifest>,
    step: usize,
    warnings: &mut Warnings,
) -> Result<(), String> {
    let p = host.provision();
    let need = || m.ok_or_else(|| format!("{module}'s module.yaml is gone"));
    match STEPS[step] {
        "started" => p
            .audit_required(&provision::event(
                "ModulePurgeStarted",
                module,
                version,
                None,
            ))
            .map_err(|e| format!("not purged: the core must record it first ({e})")),
        "backup" => {
            let dir = p.purge_backup(need()?, warnings)?;
            warnings.push(format!("{module}'s backup is {}", dir.display()));
            Ok(())
        }
        // The core refuses to purge a module it still lists as installed
        // (V4): it is told the modules first, as `remove` tells it.
        "delete" => host
            .tell_core()
            .map_err(|e| format!("quality-data was not told the modules: {e}"))
            .and_then(|()| p.delete_module(module)),
        "drop" => match need()?.database_role() {
            Some(role) => {
                let dbs: Vec<String> = need()?
                    .created_databases()
                    .map(|d| d.name.clone())
                    .collect();
                p.drop_database(&role, &dbs)
            }
            None => Ok(()),
        },
        "files" => p.purge_files(module),
        _ => {
            p.audit(&provision::event("ModulePurged", module, version, None));
            let platform = host.installed()?.0.version;
            host.report("module_purged", &platform, module, version, "");
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "purge_tests.rs"]
mod tests;
