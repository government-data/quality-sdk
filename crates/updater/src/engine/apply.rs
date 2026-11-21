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
    let apply = || -> Result<(), String> {
        let new = host.manifest(&target.package)?;
        host.put_package(&target.package, &difference(&previous.manifest, &new))?;
        host.put_version(&target.version, &target.api, &target.web)?;
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
