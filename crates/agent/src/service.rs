//! `quality-data-agent run`: the agent as a service (decision 0003 step
//! 11). It assembles the real host from the adapters, holds the agent's
//! lock, and runs the engine in rounds: each round loads the state file
//! (none: a new state at the installed version) and advances the engine
//! as the service until it has nothing to do now, then waits. The engine
//! asks the cloud every hour by itself, sooner on an "Update now".
//! Before each round the agent renews its certificate for the `internal`
//! entry point when less than 30 days are left ([`renewer`]).

use std::fs::File;
use std::path::Path;
use std::time::Duration;

use updater::engine::{advance, Host, Who};
use updater::state::State;

use crate::api::internal_files::InternalFiles;
use crate::api::renew::{Renewal, Renewer};
use crate::api::{tls, UpdaterApi};
use crate::aws::cloud::AwsCloud;
use crate::compose::DockerCompose;
use crate::config::Config;
use crate::host::{Adapters, AgentHost, SystemClock};
use crate::package_files::PackageFiles;
use crate::paths::Paths;
use crate::postgres::Postgres;

/// How long a round waits for the next when nothing is left to do.
pub const EVERY: Duration = Duration::from_secs(60);

/// The real host of the installation at `paths`, from its settings.
pub fn assemble(paths: &Paths) -> Result<AgentHost, String> {
    let config = Config::load(&paths.config_file())?;
    let api = config.api.as_ref().ok_or_else(|| {
        format!(
            "{}: run needs [api] url, quality-data's updater/v1: without it no version \
             could be verified",
            paths.config_file().display()
        )
    })?;
    let adapters = Adapters {
        cloud: Box::new(AwsCloud::from_config(&config, paths)?),
        compose: Box::new(DockerCompose::new(paths)),
        database: Box::new(Postgres::new(paths, &config.helper_image())),
        packages: Box::new(PackageFiles::new(paths)),
        api: Box::new(UpdaterApi::from_settings(api, paths)?),
        clock: Box::new(SystemClock),
    };
    Ok(AgentHost::new(adapters, paths.state_file()))
}

/// The renewal of the agent's certificate for the `internal` entry point,
/// through the API the settings name.
pub fn renewer(paths: &Paths) -> Result<Renewer, String> {
    let config = Config::load(&paths.config_file())?;
    let api = config
        .api
        .ok_or_else(|| format!("{}: no [api] url", paths.config_file().display()))?;
    let files = InternalFiles::new(paths);
    let transport = tls::Internal::new(files.clone(), api.connect)?;
    Ok(Renewer::new(&api.url, Box::new(transport), files))
}

/// Renew when due, and say what happened; a failure is tried again at the
/// next round (the certificate in use keeps working meanwhile).
pub fn renew(renewer: &Renewer, now: chrono::DateTime<chrono::Utc>) {
    match renewer.renew_if_due(now) {
        Ok(Renewal::NotDue { not_after }) => {
            tracing::debug!(%not_after, "the internal certificate is not due for renewal");
        }
        Ok(Renewal::Renewed { not_after }) => {
            tracing::info!(%not_after, "the internal certificate was renewed");
        }
        Err(err) => tracing::warn!("the internal certificate could not be renewed: {err}"),
    }
}

/// The audit events the core has not taken yet, sent in order (decision
/// 0009): at start and before each round while any waits.
pub fn flush_audit(paths: &Paths) {
    let outbox = crate::api::modules::Outbox::new(paths);
    if !outbox.path.exists() {
        return;
    }
    let sent = Config::load(&paths.config_file()).and_then(|config| {
        let settings = config
            .api
            .ok_or_else(|| format!("{}: no [api] url", paths.config_file().display()))?;
        outbox.flush(&UpdaterApi::from_settings(&settings, paths)?)
    });
    match sent {
        Ok(0) => {}
        Ok(n) => tracing::info!(events = n, "waiting audit events sent"),
        Err(err) => tracing::warn!("the audit outbox could not be sent: {err}"),
    }
}

/// The agent's lock, held while it lives: one agent per installation.
pub fn lock(paths: &Paths) -> Result<File, String> {
    let path = paths.data_dir().join("updater/agent.lock");
    let what = |e: std::io::Error| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(what)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(what)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(format!(
            "another agent runs on this installation ({} is locked)",
            path.display()
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(what(e)),
    }
}

/// One round: the state file (or a new state at the installed version),
/// then the engine until it has nothing to do now.
pub fn round(host: &dyn Host, state_file: &Path) -> Result<(), String> {
    let mut state = match State::load(state_file)? {
        Some(s) => s,
        None => {
            let installed = host.installed()?;
            tracing::info!(version = %installed.version, "a new state at the installed version");
            let s = State::new(&installed.version);
            host.save(&s)?;
            s
        }
    };
    advance(host, &mut state, Who::Service)
}

/// Rounds, `every` apart, `rounds` of them (`None`: for as long as it
/// lives). A round that fails is logged; the next one runs.
pub fn run(host: &dyn Host, state_file: &Path, every: Duration, rounds: Option<u64>) {
    run_with(host, state_file, every, rounds, &mut || ());
}

/// [`run`], doing `before` ahead of each round (the certificate's renewal).
pub fn run_with(
    host: &dyn Host,
    state_file: &Path,
    every: Duration,
    rounds: Option<u64>,
    before: &mut dyn FnMut(),
) {
    let mut done = 0;
    loop {
        before();
        if let Err(err) = round(host, state_file) {
            tracing::error!("the round failed: {err}");
        }
        done += 1;
        if rounds.is_some_and(|n| done >= n) {
            return;
        }
        host.wait(every);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_agent_holds_the_lock() {
        let r = std::env::temp_dir().join(format!("agent-lock-{}", std::process::id()));
        let paths = Paths::under(&r);
        let held = lock(&paths).unwrap();
        assert!(lock(&paths).unwrap_err().contains("another agent runs"));
        drop(held);
        lock(&paths).unwrap();
        std::fs::remove_dir_all(r).unwrap();
    }

    #[test]
    fn run_needs_the_api() {
        let r = std::env::temp_dir().join(format!("agent-assemble-{}", std::process::id()));
        let paths = Paths::under(&r);
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let text = "installation = \"inst-0001\"\nchannel = \"stable\"\n[aws]\n\
            region = \"us-east-1\"\nbucket = \"releases\"\n\
            trust_anchor_arn = \"arn:aws:rolesanywhere:us-east-1:1:trust-anchor/a\"\n\
            profile_arn = \"arn:aws:rolesanywhere:us-east-1:1:profile/p\"\n\
            role_arn = \"arn:aws:iam::1:role/r\"\n";
        std::fs::write(paths.config_file(), text).unwrap();
        assert!(assemble(&paths).unwrap_err().contains("run needs [api]"));
        std::fs::remove_dir_all(r).unwrap();
    }
}
