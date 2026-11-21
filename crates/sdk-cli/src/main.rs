//! `quality-sdk`: enrol an installation, sign its enrolment, revoke it,
//! publish a platform version (decision 0001).

use std::path::Path;
use std::process::ExitCode;

use agent::paths::Paths;
use sdk_cli::args::Args;
use sdk_cli::pki;

const USAGE: &str = "\
usage: quality-sdk <command> [options]

commands:
  enrol --installation ID [--root DIR]
      On the installation's server: make its P-256 key (kept there, 0600,
      /etc/quality-data/enrolment/private-key.pem) and its certificate
      request (enrolment/request.csr, OU=installation, CN=ID), printed for
      the CA's holder. Run again, it keeps the key.

options:
  --root DIR   the installation's paths under DIR (development)";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return usage("a command is needed");
    };
    let result = match command.as_str() {
        "enrol" => Args::parse(args, &["installation", "root"]).and_then(|a| enrol(&a)),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "-V" | "--version" => {
            println!("quality-sdk {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        other => return usage(&format!("unknown command {other:?}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn usage(error: &str) -> ExitCode {
    eprintln!("error: {error}\n\n{USAGE}");
    ExitCode::from(2)
}

fn paths(args: &Args) -> Paths {
    args.get("root")
        .map_or_else(Paths::system, |root| Paths::under(Path::new(root)))
}

fn enrol(args: &Args) -> Result<(), String> {
    let paths = paths(args);
    let done = sdk_cli::enrol::run(&paths, args.required("installation")?)?;
    let key = pki::path(&paths.private_key());
    if done.new_key {
        eprintln!("key       {key} (new, 0600; it never leaves this server)");
    } else {
        eprintln!("key       {key} (kept)");
    }
    eprintln!("request   {}", pki::path(&paths.certificate_request()));
    eprintln!("Send the request below to the CA's holder for `quality-sdk sign-enrolment`;");
    eprintln!(
        "put the certificate they return at {}.",
        pki::path(&paths.certificate())
    );
    print!("{}", done.request_pem);
    Ok(())
}
