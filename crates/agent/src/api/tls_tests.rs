//! mTLS against a local server that requires a client certificate of the
//! test CA, all made here: no real CA or installation.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use super::https;
use crate::api::UpdaterApi;
use crate::host::Api;

/// A CA, and a certificate for `name` it signed with its key.
struct Pki {
    ca: CertifiedIssuer<'static, KeyPair>,
}

impl Pki {
    fn new(cn: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, cn);
        let ca = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
        Self { ca }
    }

    /// A leaf for the names `sans`: its certificate's DER and PEM, its key.
    fn leaf(&self, sans: &[&str]) -> (CertificateDer<'static>, String, KeyPair) {
        let key = KeyPair::generate().unwrap();
        let sans: Vec<String> = sans.iter().map(|s| (*s).to_string()).collect();
        let cert = CertificateParams::new(sans)
            .unwrap()
            .signed_by(&key, &self.ca)
            .unwrap();
        (cert.der().clone(), cert.pem(), key)
    }
}

/// A server for `localhost` with a certificate of `server_ca` that requires
/// a client certificate of `client_ca` and answers one version.
fn server(server_ca: &Pki, client_ca: &Pki) -> u16 {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(client_ca.ca.der().clone()).unwrap();
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .unwrap();
    let (der, _, key) = server_ca.leaf(&["localhost"]);
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![der], key)
        .unwrap();
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for tcp in listener.incoming().flatten() {
            let conn = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            let mut line = String::new();
            // A client the verifier refuses fails here.
            let mut reader = BufReader::new(&mut tls);
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {}
            let body = r#"{"version":"1.4.0"}"#;
            let _ = write!(
                tls,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = tls.flush();
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    port
}

/// The agent's files: the CA it trusts, its certificate and key.
fn files(name: &str, trusted: &Pki, issuer: &Pki) -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("agent-tls-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (_, pem, key) = issuer.leaf(&["agent"]);
    let paths = (
        dir.join("ca.pem"),
        dir.join("certificate.pem"),
        dir.join("key.pem"),
    );
    std::fs::write(&paths.0, trusted.ca.pem()).unwrap();
    std::fs::write(&paths.1, pem).unwrap();
    std::fs::write(&paths.2, key.serialize_pem()).unwrap();
    paths
}

fn version(port: u16, (ca, cert, key): &(PathBuf, PathBuf, PathBuf)) -> Result<String, String> {
    let api = UpdaterApi::new(
        &format!("https://localhost:{port}"),
        Box::new(https(ca, cert, key)?),
    );
    api.version()
}

#[test]
fn the_agent_reaches_a_server_of_its_ca_with_its_certificate() {
    let installation = Pki::new("installation CA");
    let port = server(&installation, &installation);
    let f = files("ok", &installation, &installation);
    assert_eq!(version(port, &f).unwrap(), "1.4.0");
}

#[test]
fn a_server_of_another_ca_or_a_certificate_it_did_not_issue_is_refused() {
    let installation = Pki::new("installation CA");
    let other = Pki::new("other CA");
    // The server's certificate is not the installation's CA's.
    let port = server(&other, &installation);
    let f = files("server", &installation, &installation);
    assert!(version(port, &f).unwrap_err().contains("does not answer"));
    // The agent's certificate is not one the server's CA issued.
    let port = server(&installation, &installation);
    let f = files("client", &installation, &other);
    assert!(version(port, &f).is_err());
}

#[test]
fn missing_or_empty_files_are_refused() {
    let missing = Path::new("/nonexistent/ca.pem");
    let err = https(missing, missing, missing).unwrap_err();
    assert!(err.contains("/nonexistent/ca.pem"), "{err}");
    let installation = Pki::new("installation CA");
    let (ca, cert, key) = files("empty", &installation, &installation);
    std::fs::write(&cert, "").unwrap();
    assert!(https(&ca, &cert, &key)
        .unwrap_err()
        .contains("must each hold a PEM certificate"));
}
