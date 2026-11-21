//! HTTPS to quality-data's `internal` entry point: mTLS with the
//! installation's CA (decisions 0002 "Traefik: the one door" and 0003). Only
//! that CA is trusted for the server, never the public roots, and the agent
//! presents its own certificate issued by it.

use std::path::Path;
use std::time::Duration;

use aws_client::http::Https;
use ureq::tls::{Certificate, ClientCert, PemItem, PrivateKey, RootCerts, TlsConfig};

/// The `updater/v1` calls are small: one that takes longer is not answering.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A transport to the `internal` entry point: `ca` is the installation's CA
/// certificate, `certificate` the agent's chain and `private_key` its key
/// (PEM files).
pub fn https(ca: &Path, certificate: &Path, private_key: &Path) -> Result<Https, String> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let pem_error = |p: &Path, e: ureq::Error| format!("{}: {e}", p.display());
    let roots = certificates(&read(ca)?).map_err(|e| pem_error(ca, e))?;
    let chain = certificates(&read(certificate)?).map_err(|e| pem_error(certificate, e))?;
    if roots.is_empty() || chain.is_empty() {
        return Err(format!(
            "{} and {} must each hold a PEM certificate",
            ca.display(),
            certificate.display()
        ));
    }
    let key = PrivateKey::from_pem(&read(private_key)?).map_err(|e| pem_error(private_key, e))?;
    let tls = TlsConfig::builder()
        .root_certs(RootCerts::new_with_certs(&roots))
        .client_cert(Some(ClientCert::new_with_certs(&chain, key)))
        .build();
    let config = ureq::Agent::config_builder()
        .tls_config(tls)
        .https_only(true)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(TIMEOUT))
        .build();
    Ok(Https::with_agent(config.into()))
}

/// Every certificate of a PEM file.
fn certificates(pem: &[u8]) -> Result<Vec<Certificate<'static>>, ureq::Error> {
    let mut out = Vec::new();
    for item in ureq::tls::parse_pem(pem) {
        if let PemItem::Certificate(c) = item? {
            out.push(c);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "tls_tests.rs"]
mod tests;
