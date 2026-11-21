//! When the new version does not work: go back to the previous one on the
//! same database, restore the backup if that fails too, and critical if
//! nothing works.

use std::path::PathBuf;

use super::{difference, enter, finish, leave_maintenance, outcome, Host};
use crate::state::{OutcomeKind, Phase, Previous, State, Target};

fn mark_failed(s: &mut State, version: &str) {
    if !s.failed.iter().any(|v| v == version) {
        s.failed.push(version.into());
    }
}

pub(super) fn roll_back(
    host: &dyn Host,
    s: &mut State,
    target: Target,
    previous: Previous,
    backup: PathBuf,
    reason: String,
) -> Result<bool, String> {
    tracing::warn!(version = %target.version, "{reason}: going back to {}", previous.version);
    // Every module back to its previous package. A module only the new
    // version has keeps its files (a removed module keeps its data); it is
    // not among the previous version's modules, so it is not put.
    let go_back = || -> Result<(), String> {
        for saved in &previous.packages {
            // What the new one brought and the previous one did not have.
            let new = target
                .packages
                .iter()
                .find(|p| p.module == saved.module)
                .and_then(|p| host.manifest(&p.path).ok())
                .unwrap_or_default();
            let remove = difference(&new, &saved.manifest);
            host.put_package(&saved.module, &saved.path, &remove)?;
        }
        host.put_version(&previous.version, &previous.modules)?;
        host.start()?;
        host.healthy(None)
    };
    match go_back() {
        Ok(()) => {
            leave_maintenance(host);
            mark_failed(s, &target.version);
            s.phase = Phase::Idle;
            let o = outcome(
                s,
                OutcomeKind::RolledBack,
                &target.version,
                format!(
                    "{reason}. It went back to {} on the same database; nothing was lost.",
                    previous.version
                ),
                host.now(),
            );
            finish(host, s, o)?;
            Ok(false)
        }
        Err(other) => enter(
            host,
            s,
            Phase::Restoring {
                target,
                previous,
                backup,
                reason: format!("{reason}; the previous one did not answer either ({other})"),
            },
        ),
    }
}

pub(super) fn restore(
    host: &dyn Host,
    s: &mut State,
    target: Target,
    previous: Previous,
    backup: PathBuf,
    reason: String,
) -> Result<bool, String> {
    tracing::warn!("{reason}: restoring backup {}", backup.display());
    let restore = || -> Result<(), String> {
        host.restore_database(&backup)?;
        host.start()?;
        host.healthy(None)
    };
    match restore() {
        Ok(()) => {
            leave_maintenance(host);
            mark_failed(s, &target.version);
            s.phase = Phase::Idle;
            let o = outcome(
                s,
                OutcomeKind::Restored,
                &target.version,
                format!(
                    "{reason}. The backup taken before updating ({}) was restored and {} is back.",
                    backup.display(),
                    previous.version
                ),
                host.now(),
            );
            finish(host, s, o)?;
            Ok(false)
        }
        Err(other) => {
            let detail = format!(
                "{reason}; restoring the backup did not work either ({other}). Backup: {}",
                backup.display()
            );
            let o = outcome(
                s,
                OutcomeKind::Critical,
                &target.version,
                detail.clone(),
                host.now(),
            );
            s.phase = Phase::Critical {
                target,
                previous,
                backup,
                reason: detail,
            };
            finish(host, s, o)?;
            Ok(false)
        }
    }
}
