//! Signing an installation's request with a test CA made here: nothing
//! comes from a real CA.

mod common;

use std::time::Duration;

use agent::paths::Paths;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{DerSignature, Signature, SigningKey, VerifyingKey};
use p256::pkcs8::{DecodePublicKey, EncodePrivateKey, LineEnding};
use x509_cert::builder::{Builder, RequestBuilder};
use x509_cert::certificate::Certificate;
use x509_cert::der::oid::db::rfc5280;
use x509_cert::der::{DecodePem, Encode, EncodePem};
use x509_cert::ext::pkix::{BasicConstraints, ExtendedKeyUsage};

use common::{ca, dir, name, test_ca};
use sdk_cli::pki::{public_key_info, subject_values, Ca};
use sdk_cli::{enrol, sign};

#[test]
fn a_signed_enrolment_names_the_installation_and_is_the_cas() {
    let d = dir("sign");
    let paths = Paths::under(&d);
    let request = enrol::run(&paths, "inst-0001").unwrap().request_pem;
    let ca = ca(&d);
    let signed = sign::sign(&ca, &request, 30).unwrap();
    assert_eq!(signed.installation, "inst-0001");
    assert_eq!(signed.serial.len(), 32);

    let cert = Certificate::from_pem(signed.certificate_pem.as_bytes()).unwrap();
    let tbs = &cert.tbs_certificate;
    assert_eq!(tbs.issuer, *ca.subject());
    let (ou, cn) = subject_values(&tbs.subject);
    assert_eq!(
        (ou, cn),
        (vec!["installation".into()], vec!["inst-0001".into()])
    );
    // The certificate holds the installation's key, and the CA signed it.
    let key =
        sdk_cli::pki::signing_key_from_pem(&std::fs::read_to_string(paths.private_key()).unwrap())
            .unwrap();
    assert_eq!(
        tbs.subject_public_key_info,
        public_key_info(key.verifying_key()).unwrap()
    );
    let ca_key =
        VerifyingKey::from_public_key_der(&ca.public_key().unwrap().to_der().unwrap()).unwrap();
    let signature = Signature::from_der(cert.signature.as_bytes().unwrap()).unwrap();
    ca_key.verify(&tbs.to_der().unwrap(), &signature).unwrap();
    // An end entity for client authentication.
    let extensions = tbs.extensions.as_ref().unwrap();
    let find = |oid| extensions.iter().find(|e| e.extn_id == oid).unwrap();
    let bc: BasicConstraints = x509_cert::der::Decode::from_der(
        find(rfc5280::ID_CE_BASIC_CONSTRAINTS).extn_value.as_bytes(),
    )
    .unwrap();
    assert!(!bc.ca);
    let eku = find(rfc5280::ID_CE_EXT_KEY_USAGE);
    assert!(!eku.critical, "as the certificates Roles Anywhere accepted");
    let eku: ExtendedKeyUsage =
        x509_cert::der::Decode::from_der(eku.extn_value.as_bytes()).unwrap();
    assert_eq!(eku.0, [rfc5280::ID_KP_CLIENT_AUTH]);
    let days =
        tbs.validity.not_after.to_unix_duration() - tbs.validity.not_before.to_unix_duration();
    assert_eq!(days, Duration::from_secs(30 * 86_400));
    std::fs::remove_dir_all(d).unwrap();
}

/// A request signed by `key` for `subject`.
fn request(subject: &str, key: &SigningKey) -> String {
    RequestBuilder::new(name(subject), key)
        .unwrap()
        .build::<DerSignature>()
        .unwrap()
        .to_pem(LineEnding::LF)
        .unwrap()
}

#[test]
fn a_request_without_ou_installation_or_one_cn_is_refused() {
    let d = dir("refused");
    let ca = ca(&d);
    let key = SigningKey::from_bytes(&[9; 32].into()).unwrap();
    for (subject, why) in [
        ("CN=inst-0001,OU=other", "OU=installation"),
        ("CN=inst-0001", "OU=installation"),
        (
            "CN=inst-0001,OU=installation,OU=installation",
            "OU=installation",
        ),
        ("OU=installation", "one CN"),
        ("CN=a,CN=b,OU=installation", "one CN"),
        ("CN=../x,OU=installation", "installation \"../x\""),
    ] {
        let err = sign::sign(&ca, &request(subject, &key), 30).unwrap_err();
        assert!(err.contains(why), "{subject}: {err}");
    }
    assert!(sign::sign(&ca, "not a request", 30).is_err());
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_request_that_does_not_hold_its_key_is_refused() {
    let d = dir("forged");
    let ca = ca(&d);
    let key = SigningKey::from_bytes(&[9; 32].into()).unwrap();
    let pem = request("CN=inst-0001,OU=installation", &key);
    let mut req = x509_cert::request::CertReq::from_pem(pem.as_bytes()).unwrap();
    // Another key put in the request after it was signed.
    let other = SigningKey::from_bytes(&[8; 32].into()).unwrap();
    req.info.public_key = public_key_info(other.verifying_key()).unwrap();
    let forged = req.to_pem(LineEnding::LF).unwrap();
    let err = sign::sign(&ca, &forged, 30).unwrap_err();
    assert!(err.contains("does not verify"), "{err}");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_ca_key_that_is_not_its_certificates_or_a_validity_past_the_ca_is_refused() {
    let d = dir("ca");
    let (cert, _) = test_ca(&d, 10);
    let other = d.join("other.key");
    let key = SigningKey::from_bytes(&[4; 32].into()).unwrap();
    std::fs::write(&other, key.to_pkcs8_pem(LineEnding::LF).unwrap().as_bytes()).unwrap();
    assert!(Ca::load(&cert, &other)
        .unwrap_err()
        .contains("not the CA certificate's key"));

    let ca = Ca::load(&cert, &d.join("ca-3.key")).unwrap();
    let pem = request("CN=inst-0001,OU=installation", &key);
    assert!(sign::sign(&ca, &pem, 11)
        .unwrap_err()
        .contains("past the CA"));
    assert!(sign::sign(&ca, &pem, 0).is_err());
    sign::sign(&ca, &pem, 9).unwrap();
    std::fs::remove_dir_all(d).unwrap();
}
