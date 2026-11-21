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
//!
//! [`Internal`] reads the files again whenever one of them was replaced (a
//! renewal), so a long-running agent presents its new certificate.

use std::net::SocketAddr;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use aws_client::http::{Https, Request, Response, Transport};
use ureq::config::Config;
use ureq::http::Uri;
use ureq::tls::{Certificate, ClientCert, PemItem, PrivateKey, RootCerts, TlsConfig};
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use super::internal_files::InternalFiles;

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
    agent(ca, certificate, private_key, connect).map(Https::with_agent)
}

/// The ureq agent [`https`] sends through.
fn agent(
    ca: &Path,
    certificate: &Path,
    private_key: &Path,
    connect: Option<SocketAddr>,
) -> Result<ureq::Agent, String> {
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
    Ok(match connect {
        Some(addr) => ureq::Agent::with_parts(config, DefaultConnector::new(), Fixed(addr)),
        None => config.into(),
    })
}

/// What identifies the files' contents: each one's inode, size and time.
type Stamp = Vec<(u64, u64, i64, i64)>;

fn stamp(files: &InternalFiles) -> Stamp {
    [&files.ca, &files.certificate, &files.private_key]
        .iter()
        .map(|f| match std::fs::metadata(f) {
            Ok(m) => (m.ino(), m.len(), m.mtime(), m.mtime_nsec()),
            Err(_) => (0, 0, 0, 0),
        })
        .collect()
}

/// HTTPS to the `internal` entry point with the agent's files as they are
/// now: read again whenever one was replaced.
pub struct Internal {
    files: InternalFiles,
    connect: Option<SocketAddr>,
    agent: Mutex<(Stamp, ureq::Agent)>,
}

impl std::fmt::Debug for Internal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Internal")
            .field("files", &self.files)
            .field("connect", &self.connect)
            .finish_non_exhaustive()
    }
}

impl Internal {
    /// The files are read now: missing or wrong, it is refused at once.
    pub fn new(files: InternalFiles, connect: Option<SocketAddr>) -> Result<Self, String> {
        let now = stamp(&files);
        let agent = agent(&files.ca, &files.certificate, &files.private_key, connect)?;
        Ok(Self {
            files,
            connect,
            agent: Mutex::new((now, agent)),
        })
    }

    /// The agent for the files as they are now; when they do not read, the
    /// call fails and the next one reads them again.
    fn current(&self) -> Result<ureq::Agent, String> {
        let mut held = self.agent.lock().unwrap_or_else(|e| e.into_inner());
        let now = stamp(&self.files);
        if now != held.0 {
            let f = &self.files;
            *held = (
                now,
                agent(&f.ca, &f.certificate, &f.private_key, self.connect)?,
            );
        }
        Ok(held.1.clone())
    }
}

impl Transport for Internal {
    fn send(&self, request: Request) -> Result<Response, String> {
        Https::with_agent(self.current()?).send(request)
    }
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
