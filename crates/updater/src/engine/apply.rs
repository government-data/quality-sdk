//! Applying the new version and verifying that it answers.

use std::path::PathBuf;

use super::{difference, enter, finish, leave_maintenance, outcome, Host, MAINTENANCE};
use crate::state::{OutcomeKind, Phase, Previous, State, Target};

pub(super) fn apply(
    host: &dyn Host,
    s: &mut State,
    target: Target,
    previous: Previous,
    backup: PathBuf,
) -> Result<bool, String> {
    // So maintenance does not expire meanwhile (the API still answers).
    let _ = host.maintenance(
        Some(&format!("updating to {}", target.version)),
        MAINTENANCE,
    );
    // Every module in one step, in its order; any failure goes back on all.
    let apply = || -> Result<(), String> {
        for package in &target.packages {
            let new = host.manifest(&package.path)?;
            let remove = difference(previous.manifest_of(&package.module), &new);
            host.put_package(&package.module, &package.path, &remove)?;
        }
        host.put_version(&target.version, &target.modules)?;
        // Each new package's needs before anything starts: a failure goes
        // back as a failed start does.
        for package in &target.packages {
            host.provision(&package.module)
                .map_err(|e| format!("{}: {e}", package.module))?;
        }
        host.start()
    };
    match apply() {
        Ok(()) => enter(
            host,
            s,
            Phase::Verifying {
                target,
                previous,
                backup,
            },
        ),
        Err(reason) => enter(
            host,
            s,
            Phase::RollingBack {
                target,
                previous,
                backup,
                reason: format!("could not apply: {reason}"),
            },
        ),
    }
}

pub(super) fn verify(
    host: &dyn Host,
    s: &mut State,
    target: Target,
    previous: Previous,
    backup: PathBuf,
) -> Result<bool, String> {
    match host.healthy(Some(&target.version)) {
        Ok(()) => {
            if let Err(err) = host.install_commands() {
                tracing::warn!("could not install the commands: {err}");
            }
            leave_maintenance(host);
            let o = outcome(
                s,
                OutcomeKind::Updated,
                &target.version,
                format!("updated from {} to {}", previous.version, target.version),
                host.now(),
            );
            s.installed_version = target.version.clone();
            s.available = None;
            s.failed.retain(|v| v != &target.version);
            s.phase = Phase::Idle;
            finish(host, s, o)?;
            Ok(false)
        }
        Err(reason) => enter(
            host,
            s,
            Phase::RollingBack {
                target,
                previous,
                backup,
                reason: format!("the new version did not answer: {reason}"),
            },
        ),
    }
}
