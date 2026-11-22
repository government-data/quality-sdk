//! `quality-data-agent update [--wait]` (decision 0007 point 3): "Update
//! now" for a person on the server. It files the request through
//! quality-data (`POST /updater/v1/request`, with the agent's internal
//! certificate), as the web's "Update now" does; the running service sees
//! it in `orders` at its next round and starts at once, whatever the
//! policy's mode and window. It takes no lock: the service keeps running.
//!
//! With `--wait` it then follows the state file until the service has
//! handled that request and settled, and says how it ended.

use std::path::Path;
use std::time::Duration;

use updater::state::{Outcome, OutcomeKind, Phase, State};

use crate::api::internal_files::InternalFiles;
use crate::api::{tls, UpdaterApi};
use crate::config::Config;
use crate::paths::Paths;

/// How often `--wait` reads the state file.
pub const POLL: Duration = Duration::from_secs(5);
/// How long `--wait` waits at most: a drain may last its policy's maximum,
/// then the backup and the apply.
pub const LIMIT: Duration = Duration::from_secs(4 * 3600);

/// How a request ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The service handled it and the cloud offered nothing newer.
    NothingNewer,
    /// The attempt it started, as the state file records it.
    Attempt(Outcome),
}

impl Ended {
    /// Whether it ended well: updated, or nothing to do.
    pub fn ok(&self) -> bool {
        match self {
            Self::NothingNewer => true,
            Self::Attempt(o) => o.kind == OutcomeKind::Updated,
        }
    }

    /// What a person reads.
    pub fn describe(&self) -> String {
        match self {
            Self::NothingNewer => "nothing newer to install".into(),
            Self::Attempt(o) => {
                let kind = serde_json::to_value(o.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let detail = if o.detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", o.detail)
                };
                format!("{kind} {} (from {}){detail}", o.version, o.from)
            }
        }
    }
}

/// The `updater/v1` client with the agent's internal certificate. Unlike the
/// service's, it does not finish a renewal left halfway: the service does.
pub fn client(paths: &Paths) -> Result<UpdaterApi, String> {
    let config = Config::load(&paths.config_file())?;
    let api = config
        .api
        .ok_or_else(|| format!("{}: no [api] url", paths.config_file().display()))?;
    let transport = tls::Internal::new(InternalFiles::new(paths), api.connect)?;
    Ok(UpdaterApi::new(&api.url, Box::new(transport)))
}

/// The last attempt the state file records, read before filing a request
/// so that `--wait` knows a new one.
pub fn last_attempt(state_file: &Path) -> Option<Outcome> {
    State::load(state_file).ok().flatten().and_then(|s| s.last)
}

/// How request `id` ended, if the service handled it and settled: idle,
/// prepared or critical. A last attempt other than `before` is the one it
/// started; none means there was nothing newer.
pub fn ended(state: &State, id: &str, before: Option<&Outcome>) -> Option<Ended> {
    if state.handled_request.as_deref() != Some(id) {
        return None;
    }
    if !matches!(
        state.phase,
        Phase::Idle | Phase::Prepared { .. } | Phase::Critical { .. }
    ) {
        return None;
    }
    Some(match &state.last {
        Some(last) if Some(last) != before => Ended::Attempt(last.clone()),
        _ => Ended::NothingNewer,
    })
}

/// Follow `state_file` every `poll` until request `id` ended, for at most
/// `limit`. `sleep` waits (the tests' does not).
pub fn wait(
    state_file: &Path,
    id: &str,
    before: Option<&Outcome>,
    (poll, limit): (Duration, Duration),
    sleep: &mut dyn FnMut(Duration),
) -> Result<Ended, String> {
    let mut waited = Duration::ZERO;
    loop {
        if let Some(state) = State::load(state_file)? {
            if let Some(end) = ended(&state, id, before) {
                return Ok(end);
            }
        }
        if waited >= limit {
            return Err(format!(
                "the request {id} was not handled within {} minutes: see the service's log",
                limit.as_secs() / 60
            ));
        }
        sleep(poll);
        waited += poll;
    }
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
