//! `quality-data-agent`: the installation's update agent. `install.sh`
//! links it as `qd`, the short name a person types on the server.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use agent::config::Config;
use agent::paths::Paths;

const USAGE: &str = "\
usage: qd [--root DIR] <command>

qd is the short name of quality-data-agent, the installation's agent.

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
  renew     renew the internal certificate when due (less than 30 days
            left), or now with --force; refused while the service runs
            (stop it first)
  update    ask for an update now, for a person on the server (decision
            0007): the request goes through quality-data, as the web's
            Update now; the running service starts it at its next round,
            whatever the policy's mode and window

options:
  --root DIR           the installation's paths under DIR (development)
  --installed VERSION  for check: the version to ask an offer for (default:
                       the state file's, or 0.0.0)
  --download           for check: download each package of the offer and
                       check its SHA-256
  --every SECS         for run: how long a round waits for the next (default
                       60; the cloud is asked every hour)
  --once               for run: one round, then exit
  --force              for renew: renew now, however much is left
  --wait               for update: wait until the service handled the
                       request and say how it ended (exit status 1 if it
                       did not update)
  --version VERSION    after install: this version of the index, even paused
                       or outside its rollout (default: the newest that
                       reaches the installation)";

/// The command line.
struct Args {
    paths: Paths,
    command: Option<String>,
    installed: Option<String>,
    download: bool,
    every: Duration,
    once: bool,
    version: Option<String>,
    wait: bool,
    force: bool,
}

/// The command line read, or how to exit (help, version, a usage error).
fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, ExitCode> {
    let mut a = Args {
        paths: Paths::system(),
        command: None,
        installed: None,
        download: false,
        every: agent::service::EVERY,
        once: false,
        version: None,
        wait: false,
        force: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => match args.next() {
                Some(dir) => a.paths = Paths::under(&PathBuf::from(dir)),
                None => return Err(usage("--root needs a directory")),
            },
            "--installed" => match args.next() {
                Some(v) => a.installed = Some(v),
                None => return Err(usage("--installed needs a version")),
            },
            "--download" => a.download = true,
            "--every" => match args.next().and_then(|s| s.parse::<u64>().ok()) {
                Some(secs) if secs > 0 => a.every = Duration::from_secs(secs),
                _ => return Err(usage("--every needs a number of seconds above 0")),
            },
            "--once" => a.once = true,
            "--wait" => a.wait = true,
            "--force" => a.force = true,
            // After `install`, the version to install; otherwise the
            // agent's own.
            "--version" if a.command.as_deref() == Some("install") => match args.next() {
                Some(v) => a.version = Some(v),
                None => return Err(usage("install --version needs a version")),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return Err(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("quality-data-agent {}", env!("CARGO_PKG_VERSION"));
                return Err(ExitCode::SUCCESS);
            }
            other if a.command.is_none() => a.command = Some(other.to_string()),
            other => return Err(usage(&format!("unexpected argument {other:?}"))),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let a = match parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let paths = &a.paths;
    match a.command.as_deref() {
        Some("config") => config(paths),
        Some("check") => match agent::check::run(paths, a.installed.as_deref(), a.download) {
            Ok(()) => {
                println!("check           PASS");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("check           FAIL: {err}");
                ExitCode::FAILURE
            }
        },
        Some("install") => install(paths, a.version.as_deref()),
        Some("run") => run(paths, a.every, a.once),
        Some("update") => update(paths, a.wait),
        Some("renew") => renew(paths, a.force),
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
            println!(
                "{version} prepared, nothing started: run the first-start commands, \
                 then run `qd install` again"
            );
            ExitCode::from(4)
        }
        Ok(Done::NeedsCertificate { version }) => {
            println!(
                "{version} started; the internal certificate is missing ({}): issue it, \
                 then run `qd install` again",
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

/// "Update now" from the server: the request filed through quality-data;
/// with `wait`, followed in the state file until it ended.
fn update(paths: &Paths, wait: bool) -> ExitCode {
    use agent::update;
    let before = update::last_attempt(&paths.state_file());
    let filed = update::client(paths).and_then(|api| api.request_update());
    let id = match filed {
        Ok(id) => id,
        Err(err) if err.contains("answered 404") => {
            eprintln!("error: this quality-data does not take requests from the server ({err})");
            return ExitCode::FAILURE;
        }
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    println!("update requested ({id}): the agent starts it at its next round");
    if !wait {
        return ExitCode::SUCCESS;
    }
    let every = (update::POLL, update::LIMIT);
    match update::wait(
        &paths.state_file(),
        &id,
        before.as_ref(),
        every,
        &mut std::thread::sleep,
    ) {
        Ok(end) => {
            println!("{}", end.describe());
            if end.ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The internal certificate renewed when due, or now with `force`, under
/// the agent's lock (the service renews too).
fn renew(paths: &Paths, force: bool) -> ExitCode {
    use agent::api::renew::Renewal;
    let renewed = agent::service::lock(paths)
        .map_err(|e| format!("{e}: stop the service first (systemctl stop quality-data-agent)"))
        .and_then(|lock| Ok((lock, agent::service::renewer(paths)?)))
        .and_then(|(_lock, renewer)| {
            let done = if force {
                renewer
                    .renew_now()
                    .map(|not_after| Renewal::Renewed { not_after })
            } else {
                renewer.renew_if_due(chrono::Utc::now())
            }?;
            Ok((done, renewer.serial()?))
        });
    match renewed {
        Ok((Renewal::Renewed { not_after }, serial)) => {
            println!("renewed: certificate {serial} until {not_after}");
            ExitCode::SUCCESS
        }
        Ok((Renewal::NotDue { not_after }, serial)) => {
            println!(
                "not due: certificate {serial} until {not_after} (`qd renew --force` renews now)"
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
