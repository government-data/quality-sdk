//! `quality-sdk`: enrol an installation, sign its enrolment, revoke it,
//! publish a platform version (decision 0001).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent::paths::Paths;
use sdk_cli::args::Args;
use sdk_cli::pki::{self, Ca};

const USAGE: &str = "\
usage: quality-sdk <command> [options]

commands:
  enrol --installation ID [--root DIR]
      On the installation's server: make its P-256 key (kept there, 0600,
      /etc/quality-data/enrolment/private-key.pem) and its certificate
      request (enrolment/request.csr, OU=installation, CN=ID), printed for
      the CA's holder. Run again, it keeps the key.
  sign-enrolment --ca-cert FILE --ca-key FILE --days N [--out FILE] REQUEST
      By the CA's holder, offline: sign an installation's certificate
      request for N days. Refused without OU=installation and one CN. The
      certificate goes to FILE or standard output; its serial is printed.
  revoke --ca-cert FILE --ca-key FILE --crl FILE --trust-anchor-arn ARN
         (--serial HEX | --certificate FILE) [--name NAME]
      By the CA's holder: add the certificate to the CRL kept in FILE (made
      if absent), sign it with the CA, and import it into Roles Anywhere as
      the trust anchor's CRL NAME (default installations), or update it.
      AWS credentials come from the environment (AWS_ACCESS_KEY_ID...).

options:
  --root DIR   the installation's paths under DIR (development)";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return usage("a command is needed");
    };
    let result = match command.as_str() {
        "enrol" => Args::parse(args, &["installation", "root"]).and_then(|a| enrol(&a)),
        "sign-enrolment" => Args::parse(args, &["ca-cert", "ca-key", "days", "out"])
            .and_then(|a| sign_enrolment(&a)),
        "revoke" => Args::parse(
            args,
            &[
                "ca-cert",
                "ca-key",
                "crl",
                "trust-anchor-arn",
                "serial",
                "certificate",
                "name",
            ],
        )
        .and_then(|a| revoke(&a)),
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

fn sign_enrolment(args: &Args) -> Result<(), String> {
    let ca = Ca::load(
        Path::new(args.required("ca-cert")?),
        Path::new(args.required("ca-key")?),
    )?;
    let days: u32 = args
        .required("days")?
        .parse()
        .map_err(|_| "--days is a number of days".to_string())?;
    let request = pki::read(Path::new(args.one("a certificate request")?))?;
    let signed = sdk_cli::sign::sign(&ca, &request, days)?;
    match args.get("out").map(PathBuf::from) {
        Some(out) => {
            pki::write_replacing(&out, signed.certificate_pem.as_bytes())?;
            eprintln!("certificate  {}", pki::path(&out));
        }
        None => print!("{}", signed.certificate_pem),
    }
    eprintln!("installation {}", signed.installation);
    eprintln!(
        "serial       {} (keep it: `revoke` takes it)",
        signed.serial
    );
    Ok(())
}

fn revoke(args: &Args) -> Result<(), String> {
    use sdk_cli::revoke::{next_crl, parse_serial, upload, Uploaded};
    use x509_cert::der::DecodePem;

    let ca = Ca::load(
        Path::new(args.required("ca-cert")?),
        Path::new(args.required("ca-key")?),
    )?;
    let trust_anchor = args.required("trust-anchor-arn")?;
    let region = trust_anchor
        .split(':')
        .nth(3)
        .filter(|r| !r.is_empty())
        .ok_or("--trust-anchor-arn is not a Roles Anywhere ARN")?;
    let serial = match (args.get("serial"), args.get("certificate")) {
        (Some(hex), None) => parse_serial(hex)?,
        (None, Some(file)) => {
            let pem = pki::read(Path::new(file))?;
            let cert = x509_cert::Certificate::from_pem(pem.as_bytes())
                .map_err(|e| format!("{file} is not a PEM certificate: {e}"))?;
            if cert.tbs_certificate.issuer != *ca.subject() {
                return Err(format!("{file} was not issued by this CA"));
            }
            cert.tbs_certificate.serial_number
        }
        _ => return Err("give --serial or --certificate, one of them".into()),
    };
    let crl_file = PathBuf::from(args.required("crl")?);
    let previous = crl_file
        .exists()
        .then(|| pki::read(&crl_file))
        .transpose()?;
    let next = next_crl(
        &ca,
        previous.as_deref(),
        &serial,
        std::time::SystemTime::now(),
    )?;
    pki::write_replacing(&crl_file, next.pem.as_bytes())?;
    let listed = if next.already {
        "already listed"
    } else {
        "added"
    };
    eprintln!("serial    {} ({listed})", pki::serial_hex(&serial));
    eprintln!(
        "crl       {} (number {}, {} revoked)",
        pki::path(&crl_file),
        next.number,
        next.serials.len()
    );
    let account = std::rc::Rc::new(aws_client::client::Account::new(
        region,
        Box::new(aws_client::credentials::Environment),
        Box::new(aws_client::http::Https::default()),
    ));
    let ra = aws_client::rolesanywhere::RolesAnywhere::new(account);
    let name = args.get("name").unwrap_or("installations");
    match upload(&ra, name, trust_anchor, &next.pem).map_err(|e| format!("Roles Anywhere: {e}"))? {
        Uploaded::Imported(id) => eprintln!("imported  CRL {name} ({id}), enabled"),
        Uploaded::Updated(id) => eprintln!("updated   CRL {name} ({id}), enabled"),
    }
    eprintln!(
        "The installation gets no new credentials from now on; those it holds last up to an hour."
    );
    Ok(())
}
