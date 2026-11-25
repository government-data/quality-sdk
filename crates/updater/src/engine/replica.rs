//! A critical volume whose backup is a replica (decision 0002, as decision
//! 0012 amends it): a replica in sync is the volume's backup; with no
//! replica configured the volume is dumped and checked as any critical
//! piece; a configured replica not in sync refuses the update. It is
//! asked before entering maintenance, so a lake whose replica is behind is
//! not drained for nothing, and again just before the backup. Asking
//! changes nothing, so a step can run twice.

use super::{finish, leave_maintenance, outcome, Host};
use crate::state::{OutcomeKind, Phase, State, Target};

/// What the core says of a module's replica (`GET
/// /updater/v1/modules/{name}/replica`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicaState {
    InSync,
    Behind,
    Failed,
    NoReplica,
    Unhealthy,
    /// The core does not replicate what the module provides.
    Unknown,
}

impl ReplicaState {
    /// As the core names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InSync => "in_sync",
            Self::Behind => "behind",
            Self::Failed => "failed",
            Self::NoReplica => "no_replica",
            Self::Unhealthy => "unhealthy",
            Self::Unknown => "unknown",
        }
    }

    /// The core's name; anything else is [`Self::Unknown`].
    pub fn parse(name: &str) -> Self {
        match name {
            "in_sync" => Self::InSync,
            "behind" => Self::Behind,
            "failed" => Self::Failed,
            "no_replica" => Self::NoReplica,
            "unhealthy" => Self::Unhealthy,
            _ => Self::Unknown,
        }
    }
}

/// An installed module's critical volume backed up by a replica, and what
/// the core says of that replica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replica {
    pub module: String,
    pub volume: String,
    pub state: ReplicaState,
}

/// The volumes with no replica, which the backup dumps (decision 0012),
/// or why the update may not go ahead: the first replica configured but
/// not in sync, or that it could not be asked. A configured replica is
/// never replaced by a dump: `unknown` and a failed call refuse, since
/// whether a replica exists is not known.
pub(super) fn to_dump(host: &dyn Host) -> Result<Vec<Replica>, String> {
    let replicas = host
        .replicas()
        .map_err(|err| format!("whether the replica is in sync is not known ({err})"))?;
    if let Some(r) = replicas
        .iter()
        .find(|r| !matches!(r.state, ReplicaState::InSync | ReplicaState::NoReplica))
    {
        return Err(format!(
            "{}'s volume {} has no replica in sync ({})",
            r.module,
            r.volume,
            r.state.as_str()
        ));
    }
    Ok(replicas
        .into_iter()
        .filter(|r| r.state == ReplicaState::NoReplica)
        .collect())
}

/// Refuse the update: back to prepared (leaving maintenance if it was in
/// it), the outcome `failed`, and a `replica_refused` report to the cloud
/// unless the last outcome was this very refusal.
pub(super) fn refuse(
    host: &dyn Host,
    s: &mut State,
    target: Target,
    why: &str,
    in_maintenance: bool,
) -> Result<(), String> {
    if in_maintenance {
        leave_maintenance(host);
    }
    let detail = format!("not updated: {why}");
    let repeated = s.last.as_ref().is_some_and(|l| {
        l.kind == OutcomeKind::Failed && l.version == target.version && l.detail == detail
    });
    let version = target.version.clone();
    s.phase = Phase::Prepared { target };
    let o = outcome(s, OutcomeKind::Failed, &version, detail, host.now());
    finish(host, s, o.clone())?;
    if !repeated {
        host.report_replica_refused(&o);
    }
    Ok(())
}
