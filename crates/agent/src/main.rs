//! `quality-data-agent`: the installation's update agent.

use std::path::PathBuf;
use std::process::ExitCode;

use agent::config::Config;
use agent::paths::Paths;

const USAGE: &str = "\
usage: quality-data-agent [--root DIR] <command>

commands:
  config    read and check /etc/quality-data/agent.toml

options:
  --root DIR  the installation's paths under DIR (development)";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut paths = Paths::system();
    let mut command = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => match args.next() {
                Some(dir) => paths = Paths::under(&PathBuf::from(dir)),
                None => return usage("--root needs a directory"),
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
