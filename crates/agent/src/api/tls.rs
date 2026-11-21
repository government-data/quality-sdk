//! HTTPS to quality-data's `internal` entry point: mTLS with the
//! installation's CA (decisions 0002 "Traefik: the one door" and 0003). Only
//! that CA is trusted for the server, never the public roots, and the agent
//! presents its own certificate issued by it.
//!
//! With `connect` (`[api] connect`, decision 0003 point 4), every
//! connection opens at that address whatever the URL's host resolves to;
//! the URL's host stays the TLS server name (SNI and the name checked in
//! the server's certificate) and the `Host` header. `/etc/hosts` is never
//! touched.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use aws_client::http::Https;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::tls::{Certificate, ClientCert, PemItem, PrivateKey, RootCerts, TlsConfig};
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

/// The `updater/v1` calls are small: one that takes longer is not answering.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A transport to the `internal` entry point: `ca` is the installation's CA
/// certificate, `certificate` the agent's chain and `private_key` its key
/// (PEM files). `connect`: where to connect instead of resolving the URL's
/// host.
pub fn https(
    ca: &Path,
    certificate: &Path,
    private_key: &Path,
    connect: Option<SocketAddr>,
) -> Result<Https, String> {
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
    let agent = match connect {
        Some(addr) => ureq::Agent::with_parts(config, DefaultConnector::new(), Fixed(addr)),
        None => config.into(),
    };
    Ok(Https::with_agent(agent))
}

/// Resolves every host to one address: `[api] connect`.
#[derive(Debug)]
struct Fixed(SocketAddr);

impl Resolver for Fixed {
    fn resolve(
        &self,
        _uri: &Uri,
        _config: &Config,
        _timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let mut addrs = self.empty();
        addrs.push(self.0);
        Ok(addrs)
    }
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
