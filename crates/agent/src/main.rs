//! `quality-data-agent`: the installation's update agent. `install.sh`
//! links it as `qd`, the short name a person types on the server.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use agent::config::Config;
use agent::paths::Paths;

/// `qd --help`.
const USAGE: &str = include_str!("usage.txt");

/// The command line.
struct Args {
    paths: Paths,
    command: Option<String>,
    installation: Option<String>,
    installed: Option<String>,
    download: bool,
    every: Duration,
    once: bool,
    version: Option<String>,
    wait: bool,
    force: bool,
    /// After `module`: the action and the module's name.
    rest: Vec<String>,
}

/// The command line read, or how to exit (help, version, a usage error).
fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, ExitCode> {
    let mut a = Args {
        paths: Paths::system(),
        command: None,
        installation: None,
        installed: None,
        download: false,
        every: agent::service::EVERY,
        once: false,
        version: None,
        wait: false,
        force: false,
        rest: Vec::new(),
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => match args.next() {
                Some(dir) => a.paths = Paths::under(&PathBuf::from(dir)),
                None => return Err(usage("--root needs a directory")),
            },
            "--installation" => match args.next() {
                Some(id) => a.installation = Some(id),
                None => return Err(usage("--installation needs an id")),
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
                println!("{}", USAGE.trim_end());
                return Err(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("quality-data-agent {}", env!("CARGO_PKG_VERSION"));
                return Err(ExitCode::SUCCESS);
            }
            other if a.command.is_none() => a.command = Some(other.to_string()),
            other if a.command.as_deref() == Some("module") => a.rest.push(other.to_string()),
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
        Some("enrol") => enrol(paths, a.installation.as_deref()),
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
        Some("module") => module(paths, &a.rest),
        Some(other) => usage(&format!("unknown command {other:?}")),
        None => usage("a command is needed"),
    }
}

fn usage(error: &str) -> ExitCode {
    eprintln!("error: {error}\n\n{}", USAGE.trim_end());
    ExitCode::from(2)
}

/// The enrolment key, made once, and its request, printed for the operator.
fn enrol(paths: &Paths, installation: Option<&str>) -> ExitCode {
    let Some(installation) = installation else {
        return usage("enrol needs --installation ID");
    };
    let done = match agent::enrol::run(paths, installation) {
        Ok(done) => done,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    let key = paths.private_key();
    if done.new_key {
        eprintln!(
            "key       {} (new, 0600; it never leaves this server)",
            key.display()
        );
    } else {
        eprintln!("key       {} (kept)", key.display());
    }
    eprintln!("request   {}", paths.certificate_request().display());
    eprintln!("Send the request below to the operator for `qsdk sign-enrolment`;");
    eprintln!(
        "put the certificate they return at {}.",
        paths.certificate().display()
    );
    print!("{}", done.request_pem);
    ExitCode::SUCCESS
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
        let registry = agent::service::registry(paths)?;
        Ok((lock, host, renewer, registry))
    });
    let (_lock, host, renewer, registry) = match started {
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
    // and again after a failure), and send the audit events it has not
    // taken yet; hand the clusters' registry token when due, and at once
    // after quality-data was told the modules anew (decision 0013).
    let supply = agent::registry::Supply::default();
    let mut before = || {
        agent::service::renew(&renewer, chrono::Utc::now());
        host.tell_modules_if_untold();
        if host.take_told() {
            supply.again_now();
        }
        agent::service::supply_registry(&supply, &registry, chrono::Utc::now());
        agent::service::flush_audit(paths);
        agent::service::disable_left_clients(paths);
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

/// `module add|remove|rotate NAME` and `module purge NAME [--confirm
/// NAME]`, under the agent's lock (decisions 0002 and 0009).
fn module(paths: &Paths, rest: &[String]) -> ExitCode {
    use agent::modules::{adapters::Adapters, add, purge::purge, remove, rotate::rotate};
    type Act = Box<dyn Fn(&Adapters, &str) -> Result<Vec<String>, String>>;
    let (action, name, act): (&String, &String, Act) = match rest {
        [action, name] if action == "purge" => {
            let typed = confirmation(name);
            (action, name, Box::new(move |h, m| purge(h, m, &typed)))
        }
        [action, name, flag, typed] if action == "purge" && flag == "--confirm" => {
            let typed = typed.clone();
            (action, name, Box::new(move |h, m| purge(h, m, &typed)))
        }
        [action, name] => match action.as_str() {
            "add" => (action, name, Box::new(|h, m| add(h, m).map(print_gets))),
            "remove" => (action, name, Box::new(|h, m| remove(h, m))),
            "rotate" => (action, name, Box::new(|h, m| rotate(h, m))),
            other => return usage(&format!("module {other:?}: add, remove, rotate or purge")),
        },
        _ => return usage("module needs add, remove, rotate or purge, and a module's name"),
    };
    logs();
    let done = agent::service::lock(paths)
        .map_err(|e| format!("{e}: stop the service first (systemctl stop quality-data-agent)"))
        .and_then(|_lock| act(&Adapters::new(paths)?, name));
    match done {
        Ok(warnings) => {
            for w in warnings {
                eprintln!("warning: {w}");
            }
            println!("{action} {name}: done");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// What an added module got, one line each; its warnings for the caller.
fn print_gets(added: agent::modules::Added) -> Vec<String> {
    for g in &added.gets {
        println!("gets {g}");
    }
    added.warnings
}

/// The module's name, typed by the person to confirm a purge.
fn confirmation(module: &str) -> String {
    use std::io::Write;
    print!("purge deletes {module} and its data after a backup; type {module} to go on: ");
    let _ = std::io::stdout().flush();
    let mut typed = String::new();
    let _ = std::io::stdin().read_line(&mut typed);
    typed.trim().to_string()
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
