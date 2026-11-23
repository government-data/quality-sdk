//! Enrolling an installation: its key stays private, its request holds it.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use agent::paths::Paths;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use x509_cert::der::{DecodePem, Encode};
use x509_cert::request::CertReq;
use x509_cert::spki::SubjectPublicKeyInfoOwned;

use agent::enrol::{self, signing_key_from_pem};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-enrol-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn enrol_makes_a_private_key_once_and_a_request_each_time() {
    let d = dir("enrol");
    let paths = Paths::under(&d);
    let first = enrol::run(&paths, "inst-0001").unwrap();
    assert!(first.new_key);
    let mode = std::fs::metadata(paths.private_key())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let key_pem = std::fs::read_to_string(paths.private_key()).unwrap();
    assert!(key_pem.starts_with("-----BEGIN PRIVATE KEY-----"));
    assert_eq!(
        std::fs::read_to_string(paths.certificate_request()).unwrap(),
        first.request_pem
    );

    let again = enrol::run(&paths, "inst-0001").unwrap();
    assert!(!again.new_key, "the key is kept");
    assert_eq!(
        std::fs::read_to_string(paths.private_key()).unwrap(),
        key_pem
    );
    assert!(enrol::run(&paths, "../other").is_err());
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn the_request_names_the_installation_and_holds_its_key() {
    let d = dir("request");
    let paths = Paths::under(&d);
    let pem = enrol::run(&paths, "inst-0001").unwrap().request_pem;
    let request = CertReq::from_pem(pem.as_bytes()).unwrap();
    assert_eq!(
        request.info.subject.to_string(),
        "CN=inst-0001,OU=installation"
    );
    let key = signing_key_from_pem(&std::fs::read_to_string(paths.private_key()).unwrap()).unwrap();
    assert_eq!(
        request.info.public_key,
        SubjectPublicKeyInfoOwned::from_key(*key.verifying_key()).unwrap()
    );
    let spki = request.info.public_key.to_der().unwrap();
    let signature = Signature::from_der(request.signature.as_bytes().unwrap()).unwrap();
    VerifyingKey::from_public_key_der(&spki)
        .unwrap()
        .verify(&request.info.to_der().unwrap(), &signature)
        .unwrap();
    std::fs::remove_dir_all(d).unwrap();
}
