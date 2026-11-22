//! Renewal against a fake of the core's `POST /internal/machines/renew`,
//! which signs the request's key with a test CA as the core does.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aws_client::http::Https;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use rcgen::{
    date_time_ymd, BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, DnType,
    IsCa, KeyPair, PublicKeyData, SignatureAlgorithm, PKCS_ECDSA_P256_SHA256,
};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use x509_cert::der::DecodePem;
use x509_cert::request::CertReq;

use super::{Renewal, Renewer, PATH};
use crate::api::internal_files::InternalFiles;

/// How the fake core answers.
#[derive(Clone, Copy, PartialEq)]
enum Core {
    Signs,
    SignsAnotherKey,
    Refuses,
}

/// A public key taken from a request: what the CA signs for.
struct Raw(Vec<u8>);

impl PublicKeyData for Raw {
    fn der_bytes(&self) -> &[u8] {
        &self.0
    }
    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_ECDSA_P256_SHA256
    }
}

fn ca() -> CertifiedIssuer<'static, KeyPair> {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(DnType::CommonName, "test CA");
    CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap()
}

/// The updater machine's certificate for `key`, ending on 1 January of
/// `year`, signed by `ca`.
fn issue(ca: &CertifiedIssuer<'static, KeyPair>, key: &impl PublicKeyData, year: i32) -> String {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, "qd");
    dn.push(DnType::OrganizationalUnitName, "updater");
    dn.push(DnType::CommonName, "agent-1");
    params.distinguished_name = dn;
    params.not_after = date_time_ymd(year, 1, 1);
    params.signed_by(key, ca).unwrap().pem()
}

/// Answer one request as `core` says; the requests' paths and bodies are
/// kept.
fn answer(core: Core, ca: &CertifiedIssuer<'static, KeyPair>, body: &str) -> (u16, String) {
    if core == Core::Refuses {
        return (401, r#"{"error":"the certificate is refused"}"#.into());
    }
    let csr: serde_json::Value = serde_json::from_str(body).unwrap();
    let request = CertReq::from_pem(csr["csr"].as_str().unwrap().as_bytes()).unwrap();
    let raw = request
        .info
        .public_key
        .subject_public_key
        .raw_bytes()
        .to_vec();
    let certificate = match core {
        Core::Signs => issue(ca, &Raw(raw), 2032),
        _ => issue(ca, &KeyPair::generate().unwrap(), 2032),
    };
    let out = serde_json::json!({
        "certificate": certificate, "ca": ca.pem(), "serial": "0a",
        "not_after": 1_956_528_000, "state": "approved",
    });
    (200, out.to_string())
}

type Asked = Arc<Mutex<Vec<(String, String)>>>;

fn fake_core(core: Core) -> (String, Asked, String) {
    let ca = ca();
    let ca_pem = ca.pem();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    let asked = Asked::default();
    let log = asked.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                match header.split_once(':') {
                    Some((k, v)) if k.eq_ignore_ascii_case("content-length") => {
                        length = v.trim().parse().unwrap();
                    }
                    None if header.trim().is_empty() => break,
                    _ => {}
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            let (status, out) = answer(core, &ca, &body);
            let what: Vec<&str> = line.split_whitespace().take(2).collect();
            log.lock().unwrap().push((what.join(" "), body));
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}",
                out.len()
            );
        }
    });
    (base, asked, ca_pem)
}

/// The agent's files, with a certificate that ends on 1 January 2030.
fn files(name: &str) -> InternalFiles {
    let d: PathBuf =
        std::env::temp_dir().join(format!("agent-renew-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let f = InternalFiles {
        ca: d.join("ca.pem"),
        certificate: d.join("certificate.pem"),
        private_key: d.join("private-key.pem"),
    };
    let ca = ca();
    let key = KeyPair::generate().unwrap();
    std::fs::write(&f.ca, ca.pem()).unwrap();
    std::fs::write(&f.certificate, issue(&ca, &key, 2030)).unwrap();
    std::fs::write(&f.private_key, key.serialize_pem()).unwrap();
    f
}

fn ends() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()
}

fn read(f: &InternalFiles) -> [String; 3] {
    [&f.ca, &f.certificate, &f.private_key].map(|p| std::fs::read_to_string(p).unwrap())
}

#[test]
fn with_30_days_or_more_left_nothing_is_asked() {
    let (base, asked, _) = fake_core(Core::Signs);
    let f = files("early");
    let r = Renewer::new(&base, Box::new(Https::default()), f.clone());
    let now = ends() - TimeDelta::days(30);
    assert_eq!(
        r.renew_if_due(now).unwrap(),
        Renewal::NotDue { not_after: ends() }
    );
    assert!(asked.lock().unwrap().is_empty());
}

#[test]
fn with_less_than_30_days_left_it_renews_with_a_new_key() {
    let (base, asked, ca_pem) = fake_core(Core::Signs);
    let f = files("due");
    let before = read(&f);
    let r = Renewer::new(&base, Box::new(Https::default()), f.clone());
    let now = ends() - TimeDelta::days(29);
    let renewed = Utc.with_ymd_and_hms(2032, 1, 1, 0, 0, 0).unwrap();
    assert_eq!(
        r.renew_if_due(now).unwrap(),
        Renewal::Renewed { not_after: renewed }
    );
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, format!("POST /api{PATH}"));
    let body: serde_json::Value = serde_json::from_str(&asked[0].1).unwrap();
    assert!(body["csr"]
        .as_str()
        .unwrap()
        .starts_with("-----BEGIN CERTIFICATE REQUEST-----"));
    assert_eq!(
        body.as_object().unwrap().len(),
        1,
        "the request is the csr only"
    );
    let after = read(&f);
    assert_eq!(after[0], ca_pem);
    assert_ne!(after[1], before[1]);
    assert_ne!(after[2], before[2], "a new key");
    assert_eq!(f.not_after().unwrap(), renewed);
    // The key is one rustls (ring) presents.
    let key = ureq::tls::PrivateKey::from_pem(after[2].as_bytes()).unwrap();
    let der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.der().to_vec()));
    rustls::crypto::ring::sign::any_ecdsa_type(&der).unwrap();
    // Renewed, it is not due any more.
    assert!(matches!(
        r.renew_if_due(now).unwrap(),
        Renewal::NotDue { .. }
    ));
}

#[test]
fn a_refusal_or_a_certificate_for_another_key_changes_nothing() {
    for (core, says) in [
        (Core::Refuses, "answered 401 the certificate is refused"),
        (Core::SignsAnotherKey, "not for the key sent"),
    ] {
        let (base, _, _) = fake_core(core);
        let f = files(if core == Core::Refuses {
            "refused"
        } else {
            "other"
        });
        let before = read(&f);
        let r = Renewer::new(&base, Box::new(Https::default()), f.clone());
        let err = r.renew_if_due(ends() - TimeDelta::days(1)).unwrap_err();
        assert!(err.contains(says), "{err}");
        assert_eq!(read(&f), before);
    }
}

#[test]
fn a_core_that_does_not_answer_changes_nothing() {
    let f = files("silent");
    let before = read(&f);
    let r = Renewer::new(
        "http://127.0.0.1:9/api",
        Box::new(Https::default()),
        f.clone(),
    );
    let err = r.renew_if_due(ends()).unwrap_err();
    assert!(err.contains("does not answer"), "{err}");
    assert_eq!(read(&f), before);
}

#[test]
fn renewing_now_renews_however_much_is_left() {
    let (base, asked, _) = fake_core(Core::Signs);
    let f = files("forced");
    let before = read(&f);
    let r = Renewer::new(&base, Box::new(Https::default()), f.clone());
    let serial = r.serial().unwrap();
    assert!(!serial.is_empty() && serial.chars().all(|c| c.is_ascii_hexdigit()));
    // Not due by the threshold: years are left.
    let now = ends() - TimeDelta::days(300);
    assert!(matches!(
        r.renew_if_due(now).unwrap(),
        Renewal::NotDue { .. }
    ));
    assert!(asked.lock().unwrap().is_empty());

    let renewed = Utc.with_ymd_and_hms(2032, 1, 1, 0, 0, 0).unwrap();
    assert_eq!(r.renew_now().unwrap(), renewed);
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert_ne!(read(&f)[1], before[1], "a new certificate");
    assert_ne!(r.serial().unwrap(), serial, "with its own serial");
}
