//! `qsdk`: enrol an installation, sign its enrolment, revoke it,
//! publish a platform version (decision 0001).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent::paths::Paths;
use sdk_cli::args::Args;
use sdk_cli::pki::{self, Ca};

const USAGE: &str = "\
usage: qsdk <command> [options]

commands:
  enrol --installation ID [--root DIR]
      On the installation's server: make its P-256 key (kept there, 0600,
      /etc/quality-data/enrolment/private-key.pem) and its certificate
      request (enrolment/request.csr, OU=installation, CN=ID), printed for
      the CA's holder. Run again, it keeps the key.
  sign-enrolment --ca-cert FILE --ca-key FILE [--days N] [--out FILE] REQUEST
      By the CA's holder, offline: sign an installation's certificate
      request for N days (default 365, never past the CA's own validity).
      Refused without OU=installation and one CN. The certificate goes to
      FILE or standard output; its serial is printed.
  revoke --ca-cert FILE --ca-key FILE [--crl FILE] --trust-anchor-arn ARN
         (--serial HEX | --certificate FILE) [--name NAME]
      By the CA's holder: add the certificate to the CRL kept in FILE
      (default ~/qd-ca/installations.crl, beside the CA key and backed up
      with it; made if absent), sign it with the CA, and import it into
      Roles Anywhere as the trust anchor's CRL NAME (default
      installations), or update it.
      AWS credentials come from the environment (AWS_ACCESS_KEY_ID...).
  publish --channel stable|patch --bucket NAME --region REGION --kms-key KEY
          [--index-key FILE] VERSION.toml
      From CI (the publish role) or by a person: add the platform version in
      VERSION.toml to index/<channel>.json with the next sequence, once
      every image it names is in the installations' ECR registry by
      digest (DescribeImages); upload
      its modules' packages, sign the index with the KMS key (the SHA-256,
      MessageType=DIGEST) and write it with its .sig once the signature
      verifies with the index key (FILE, PEM, or else KMS's public key).
      AWS credentials come from the environment.

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
        "publish" => Args::parse(
            args,
            &["channel", "bucket", "region", "kms-key", "index-key"],
        )
        .and_then(|a| publish(&a)),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "-V" | "--version" => {
            println!("qsdk {}", env!("CARGO_PKG_VERSION"));
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
    eprintln!("Send the request below to the CA's holder for `qsdk sign-enrolment`;");
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
    let days = args
        .get("days")
        .map(|d| d.parse::<u32>())
        .transpose()
        .map_err(|_| "--days is a number of days".to_string())?;
    let days = sdk_cli::sign::days_or_default(&ca, days);
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
    let crl_file = match args.get("crl") {
        Some(file) => PathBuf::from(file),
        None => sdk_cli::revoke::default_crl(std::env::var_os("HOME").as_deref())?,
    };
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

fn publish(args: &Args) -> Result<(), String> {
    use aws_client::client::{Account, Bucket};
    use aws_client::kms::Kms;
    use index_model::{Channel, IndexKey};

    let channel = match args.required("channel")? {
        "stable" => Channel::Stable,
        "patch" => Channel::Patch,
        other => return Err(format!("--channel {other:?}: stable or patch")),
    };
    let release = sdk_cli::publish::input::read(Path::new(args.one("a platform version file")?))?;
    let account = std::rc::Rc::new(Account::new(
        args.required("region")?,
        Box::new(aws_client::credentials::Environment),
        Box::new(aws_client::http::Https::default()),
    ));
    let kms = Kms::new(account.clone(), args.required("kms-key")?);
    let key = match args.get("index-key") {
        Some(file) => IndexKey::from_public_key_pem(&pki::read(Path::new(file))?),
        None => {
            let der = kms.public_key().map_err(|e| format!("KMS: {e}"))?;
            IndexKey::from_public_key_der(&der)
        }
    }
    .map_err(|e| e.to_string())?;
    let registry = aws_client::ecr::Ecr::new(account.clone());
    let bucket = Bucket::new(account, args.required("bucket")?);
    let version = release.version.version.to_string();
    let done = sdk_cli::publish::publish(
        &bucket,
        &kms,
        &registry,
        &key,
        channel,
        release,
        chrono::Utc::now(),
    )?;
    for p in &done.uploaded {
        eprintln!("package   {p} (uploaded)");
    }
    for p in &done.kept {
        eprintln!("package   {p} (already there, same SHA-256)");
    }
    eprintln!("images    every one in the installations' registry");
    eprintln!(
        "index     {} sequence {}, with {version}, signed and verified",
        done.index_key, done.sequence
    );
    Ok(())
}
