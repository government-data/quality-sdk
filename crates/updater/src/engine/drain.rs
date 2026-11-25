//! Draining what runs, in maintenance, and backing up before anything is
//! touched.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};

use super::replica::{not_in_sync, refuse};
use super::{enter, finish, leave_maintenance, outcome, Drain, Host, Who, DRAIN_POLL};
use crate::state::{OutcomeKind, Phase, Previous, SavedPackage, State, Target};

pub(super) fn drain(
    host: &dyn Host,
    s: &mut State,
    who: Who,
    target: Target,
    since: DateTime<Utc>,
    request: Option<String>,
) -> Result<bool, String> {
    let limit = match who {
        Who::Service => host.orders().policy.max_drain_secs,
        Who::Person => crate::policy::Policy::default().max_drain_secs,
    };
    loop {
        match host.drain(Duration::ZERO) {
            Ok(d) if d.drained => {
                return enter(host, s, Phase::BackingUp { target, request });
            }
            Ok(d) => {
                tracing::info!(
                    running = d.running,
                    deploying = d.deploying,
                    "waiting for running work to finish"
                );
                // So the web shows what is left; if it cannot be saved, it
                // keeps waiting anyway.
                let progress = Some(what_runs(&d));
                if s.progress != progress {
                    s.progress = progress;
                    if let Err(err) = host.save(s) {
                        tracing::debug!("could not report progress: {err}");
                    }
                }
            }
            Err(err) => tracing::warn!("could not tell what is running: {err}"),
        }
        let now = host.now();
        if (now - since).to_std().unwrap_or_default() >= Duration::from_secs(limit) {
            leave_maintenance(host);
            s.phase = Phase::Prepared {
                target: target.clone(),
            };
            let o = outcome(
                s,
                OutcomeKind::Postponed,
                &target.version,
                format!(
                    "work was still running after {} min: it is tried later",
                    limit / 60
                ),
                now,
            );
            finish(host, s, o)?;
            return Ok(false);
        }
        host.wait(DRAIN_POLL);
    }
}

/// "2 flows running · 1 deployment", for the web.
fn what_runs(d: &Drain) -> String {
    let plural = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if d.running > 0 {
        parts.push(plural(d.running, "flow running", "flows running"));
    }
    if d.deploying > 0 {
        parts.push(plural(d.deploying, "deployment", "deployments"));
    }
    if parts.is_empty() {
        "finishing what runs".into()
    } else {
        parts.join(" · ")
    }
}

pub(super) fn back_up(
    host: &dyn Host,
    s: &mut State,
    target: Target,
    _request: Option<String>,
) -> Result<bool, String> {
    if let Some(why) = not_in_sync(host) {
        refuse(host, s, target, &why, true)?;
        return Ok(false);
    }
    let prepare = || -> Result<(PathBuf, Previous), String> {
        let backup = host.back_up()?;
        let installed = host.installed()?;
        let mut packages = Vec::with_capacity(installed.modules.len());
        // Each module at its own installed version, not the platform's.
        for (module, m) in &installed.modules {
            let (path, manifest) = host.save_package(module, &m.version)?;
            packages.push(SavedPackage {
                module: module.clone(),
                path,
                manifest,
            });
        }
        Ok((
            backup,
            Previous {
                version: installed.version,
                modules: installed.modules,
                packages,
            },
        ))
    };
    match prepare() {
        Ok((backup, previous)) => enter(
            host,
            s,
            Phase::Applying {
                target,
                previous,
                backup,
            },
        ),
        Err(err) => {
            leave_maintenance(host);
            s.phase = Phase::Prepared {
                target: target.clone(),
            };
            let o = outcome(
                s,
                OutcomeKind::Failed,
                &target.version,
                format!("not updated: the backup failed ({err})"),
                host.now(),
            );
            finish(host, s, o)?;
            Ok(false)
        }
    }
}
