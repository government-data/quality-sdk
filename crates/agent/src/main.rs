//! `quality-data-agent`: the installation's update agent.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use agent::config::Config;
use agent::paths::Paths;

const USAGE: &str = "\
usage: quality-data-agent [--root DIR] <command>

commands:
  config    read and check /etc/quality-data/agent.toml
  check     check the AWS side end to end: credentials, the signed index,
            the registry and one report of kind `check` (no image is
            pulled, nothing published; packages only with --download)
  install   lay out the first platform version (decision 0004): choose,
            prepare, start; once the internal certificate exists, verify.
            Exit status 4: prepared, nothing started; run the module's
            first-start commands, then install again. Exit status 3:
            started, the internal certificate is missing; run it again
            once it is issued
  run       the agent as a service: run the update engine with the real
            host in rounds, until stopped (needs [api] in agent.toml)

options:
  --root DIR           the installation's paths under DIR (development)
  --installed VERSION  for check: the version to ask an offer for (default:
                       the state file's, or 0.0.0)
  --download           for check: download each package of the offer and
                       check its SHA-256
  --every SECS         for run: how long a round waits for the next (default
                       60; the cloud is asked every hour)
  --once               for run: one round, then exit
  --version VERSION    after install: this version of the index, even paused
                       or outside its rollout (default: the newest that
                       reaches the installation)";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut paths = Paths::system();
    let mut command = None;
    let mut installed = None;
    let mut download = false;
    let mut every = agent::service::EVERY;
    let mut once = false;
    let mut version = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => match args.next() {
                Some(dir) => paths = Paths::under(&PathBuf::from(dir)),
                None => return usage("--root needs a directory"),
            },
            "--installed" => match args.next() {
                Some(v) => installed = Some(v),
                None => return usage("--installed needs a version"),
            },
            "--download" => download = true,
            "--every" => match args.next().and_then(|s| s.parse::<u64>().ok()) {
                Some(secs) if secs > 0 => every = Duration::from_secs(secs),
                _ => return usage("--every needs a number of seconds above 0"),
            },
            "--once" => once = true,
            // After `install`, the version to install; otherwise the
            // agent's own.
            "--version" if command.as_deref() == Some("install") => match args.next() {
                Some(v) => version = Some(v),
                None => return usage("install --version needs a version"),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("quality-data-agent {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other if command.is_none() => command = Some(other.to_string()),
            other => return usage(&format!("unexpected argument {other:?}")),
        }
    }
    match command.as_deref() {
        Some("config") => config(&paths),
        Some("check") => match agent::check::run(&paths, installed.as_deref(), download) {
            Ok(()) => {
                println!("check           PASS");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("check           FAIL: {err}");
                ExitCode::FAILURE
            }
        },
        Some("install") => install(&paths, version.as_deref()),
        Some("run") => run(&paths, every, once),
        Some(other) => usage(&format!("unknown command {other:?}")),
        None => usage("a command is needed"),
    }
}

fn usage(error: &str) -> ExitCode {
    eprintln!("error: {error}\n\n{USAGE}");
    ExitCode::from(2)
}

/// Read and check the settings, and say what they point at.
fn config(paths: &Paths) -> ExitCode {
    match Config::load(&paths.config_file()) {
        Ok(c) => {
            println!("installation: {}", c.installation);
            println!("channel:      {:?}", c.channel);
            println!("region:       {}", c.aws.region);
            println!("bucket:       {}", c.aws.bucket);
            println!("state file:   {}", paths.state_file().display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The first platform version, under the agent's lock.
fn install(paths: &Paths, version: Option<&str>) -> ExitCode {
    use agent::install::{adapters, Done};
    logs();
    let done = agent::service::lock(paths).and_then(|_lock| {
        let installer = adapters::Adapters::new(paths)?;
        agent::install::install(&installer, &adapters::files(paths), version)
    });
    match done {
        Ok(Done::Installed { version }) => {
            println!("installed {version}");
            ExitCode::SUCCESS
        }
        Ok(Done::Prepared { version }) => {
            println!("{version} prepared, nothing started: run the first-start commands, then run install again");
            ExitCode::from(4)
        }
        Ok(Done::NeedsCertificate { version }) => {
            println!(
                "{version} started; the internal certificate is missing ({}): issue it,                  then run install again",
                paths.internal_certificate().display()
            );
            ExitCode::from(3)
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn logs() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::INFO)
        .init();
}

/// The agent as a service: its lock, the real host, rounds until stopped.
fn run(paths: &Paths, every: Duration, once: bool) -> ExitCode {
    logs();
    let started = agent::service::lock(paths).and_then(|lock| {
        let host = agent::service::assemble(paths)?;
        let renewer = agent::service::renewer(paths)?;
        Ok((lock, host, renewer))
    });
    let (_lock, host, renewer) = match started {
        Ok(started) => started,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        every_secs = every.as_secs(),
        "quality-data-agent runs"
    );
    let rounds = once.then_some(1);
    // Before each round: renew the internal certificate when due, and tell
    // quality-data the installed modules until it has heard them (at start,
    // and again after a failure).
    let mut before = || {
        agent::service::renew(&renewer, chrono::Utc::now());
        host.tell_modules_if_untold();
    };
    agent::service::run_with(&host, &paths.state_file(), every, rounds, &mut before);
    ExitCode::SUCCESS
}
