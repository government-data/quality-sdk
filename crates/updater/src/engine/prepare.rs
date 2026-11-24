//! Idle or prepared: ask where versions come from, prepare a new one outside
//! the window (package, images, space) and start when the moment comes.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::order::apply_order;
use super::replica::{not_in_sync, refuse};
use super::{
    enter, finish, is_newer, outcome, CloudError, Host, Offer, Who, CHECK_EVERY, MAINTENANCE,
};
use crate::policy::{Mode, Orders};
use crate::state::{Available, ModulePackage, OutcomeKind, Phase, State, Target};

/// Idle or with a prepared version: ask the cloud if due, and start if the
/// moment came.
pub(super) fn waiting(
    host: &dyn Host,
    s: &mut State,
    who: Who,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let orders = match who {
        Who::Service => host.orders(),
        Who::Person => Orders::default(),
    };
    let request = orders
        .request
        .as_ref()
        .map(|r| r.id.clone())
        .filter(|id| s.handled_request.as_deref() != Some(id));

    // Ask the cloud? Every hour, or if someone asks.
    let due = who == Who::Person
        || request.is_some()
        || s.last_check
            .is_none_or(|t| (now - t).to_std().unwrap_or_default() >= CHECK_EVERY);
    if due {
        check(host, s, now)?;
    }

    let Phase::Prepared { target } = s.phase.clone() else {
        // Nothing prepared: a request without a new version counts as handled.
        if let Some(id) = request {
            s.handled_request = Some(id);
            host.save(s)?;
        }
        return Ok(false);
    };

    if !should_start(host, s, who, &orders, &target, request.is_some(), now) {
        return Ok(false);
    }
    if let Some(why) = not_in_sync(host) {
        if let Some(id) = request {
            s.handled_request = Some(id);
        }
        refuse(host, s, target, &why, false)?;
        return Ok(false);
    }
    let reason = format!("updating to {}", target.version);
    let lasts = Duration::from_secs(orders.policy.max_drain_secs) + MAINTENANCE;
    if let Err(err) = host.maintenance(Some(&reason), lasts) {
        // Without maintenance there is no draining: it is retried next step.
        tracing::warn!("could not enter maintenance: {err}");
        return Ok(false);
    }
    tracing::info!(version = %target.version, "update starting: waiting for running work to finish");
    if let Some(id) = &request {
        s.handled_request = Some(id.clone());
    }
    enter(
        host,
        s,
        Phase::Draining {
            target,
            since: now,
            request,
        },
    )
}

/// Whether the moment came: a person, an "Update now", or the automatic
/// policy inside its window.
fn should_start(
    host: &dyn Host,
    s: &State,
    who: Who,
    orders: &Orders,
    target: &Target,
    requested: bool,
    now: DateTime<Utc>,
) -> bool {
    match who {
        Who::Person => true,
        Who::Service if requested => true,
        Who::Service => {
            let p = &orders.policy;
            p.mode == Mode::Automatic
                && !p.paused
                && !s.failed.contains(&target.version)
                && p.window.contains(now)
                && nothing_scheduled(host, p.skip_if_scheduled_within_secs)
        }
    }
}

fn nothing_scheduled(host: &dyn Host, within: u64) -> bool {
    if within == 0 {
        return true;
    }
    match host.drain(Duration::from_secs(within)) {
        Ok(d) => d.scheduled_soon == 0,
        Err(_) => false,
    }
}

/// Ask the cloud and, if there is a new version, prepare it.
fn check(host: &dyn Host, s: &mut State, now: DateTime<Utc>) -> Result<(), String> {
    s.last_check = Some(now);
    let offer = match host.check_cloud(&s.installed_version) {
        Ok(offer) => offer,
        Err(CloudError::Rejected(m) | CloudError::Network(m)) => {
            tracing::warn!("cloud: {m}");
            s.warning = Some(m);
            host.save(s)?;
            return Ok(());
        }
    };
    s.warning = None;
    let offer = installed_only(host, offer);
    // Never downgrade, even if the cloud gets it wrong.
    if !is_newer(&offer.version, &s.installed_version) {
        s.available = None;
        if matches!(s.phase, Phase::Prepared { .. }) {
            s.phase = Phase::Idle;
        }
        host.save(s)?;
        return Ok(());
    }
    s.available = Some(Available {
        version: offer.version.clone(),
        notes: offer.notes.clone(),
    });
    // That same version already prepared: nothing to download.
    if let Phase::Prepared { target } = &s.phase {
        if target.version == offer.version && target.packages.iter().all(|p| p.path.exists()) {
            host.save(s)?;
            return Ok(());
        }
    }
    host.save(s)?;
    prepare(host, s, offer, now)
}

/// Of the offered version, only the modules installed now (decision 0003,
/// the amendment of 2026-11-23): one the installation has not added is
/// left alone; `qd module add` adds it. With nothing recorded (the first
/// install) or no module recorded, every module.
fn installed_only(host: &dyn Host, mut offer: Offer) -> Offer {
    let Ok(installed) = host.installed() else {
        return offer;
    };
    if installed.modules.is_empty() {
        return offer;
    }
    let has = |m: &str| installed.modules.contains_key(m);
    offer.modules.retain(|m, _| has(m));
    offer.packages.retain(|p| has(&p.module));
    offer
}

/// Check the space, download every module's package in the order they are
/// applied and pull the images; a failure leaves it idle, with nothing
/// touched.
fn prepare(host: &dyn Host, s: &mut State, offer: Offer, now: DateTime<Utc>) -> Result<(), String> {
    let prepare = || -> Result<Vec<ModulePackage>, String> {
        let order = apply_order(&offer.packages)?;
        host.free_space()?;
        let mut packages = Vec::with_capacity(order.len());
        for package in &order {
            packages.push(ModulePackage {
                module: package.module.clone(),
                path: host.download_package(&offer, package)?,
            });
        }
        host.pull_images(&offer)?;
        Ok(packages)
    };
    match prepare() {
        Ok(packages) => {
            tracing::info!(version = %offer.version, "new version prepared");
            s.phase = Phase::Prepared {
                target: Target {
                    version: offer.version,
                    modules: offer.modules,
                    packages,
                    notes: offer.notes,
                },
            };
            host.save(s)
        }
        Err(err) => {
            s.phase = Phase::Idle;
            let o = outcome(
                s,
                OutcomeKind::Failed,
                &offer.version,
                format!("could not prepare: {err}"),
                now,
            );
            finish(host, s, o)
        }
    }
}
