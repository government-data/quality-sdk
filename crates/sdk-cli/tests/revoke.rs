//! Revoking: the CRL a test CA signs, appended to, and put into a fake
//! Roles Anywhere.

mod common;

use std::time::SystemTime;

use aws_client::fake::{account, FakeAws};
use aws_client::rolesanywhere::RolesAnywhere;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use x509_cert::crl::CertificateList;
use x509_cert::der::oid::db::rfc5280;
use x509_cert::der::{pem, Decode, Encode};

use common::{ca, dir, test_ca_with};
use sdk_cli::pki::Ca;
use sdk_cli::revoke::{next_crl, parse_serial, upload, Uploaded};

fn decode(pem_text: &str) -> CertificateList {
    let (label, der) = pem::decode_vec(pem_text.as_bytes()).unwrap();
    assert_eq!(label, "X509 CRL");
    CertificateList::from_der(&der).unwrap()
}

fn verifies(ca: &Ca, crl: &CertificateList) -> bool {
    let key =
        VerifyingKey::from_public_key_der(&ca.public_key().unwrap().to_der().unwrap()).unwrap();
    let signature = Signature::from_der(crl.signature.as_bytes().unwrap()).unwrap();
    key.verify(&crl.tbs_cert_list.to_der().unwrap(), &signature)
        .is_ok()
}

#[test]
fn the_first_crl_lists_the_serial_signed_by_the_ca_until_the_ca_ends() {
    let d = dir("revoke-first");
    let ca = ca(&d);
    let serial = parse_serial("63B43158DBBEEF0BE48E3B24CFF88B89").unwrap();
    let next = next_crl(&ca, None, &serial, SystemTime::now()).unwrap();
    assert_eq!((next.number, next.already), (1, false));
    assert_eq!(next.serials, ["63B43158DBBEEF0BE48E3B24CFF88B89"]);

    let crl = decode(&next.pem);
    let tbs = &crl.tbs_cert_list;
    assert!(verifies(&ca, &crl));
    assert_eq!(tbs.issuer, *ca.subject());
    assert_eq!(
        tbs.next_update,
        Some(ca.certificate.tbs_certificate.validity.not_after)
    );
    let ids: Vec<_> = tbs
        .crl_extensions
        .iter()
        .flatten()
        .map(|e| e.extn_id)
        .collect();
    assert_eq!(
        ids,
        [
            rfc5280::ID_CE_AUTHORITY_KEY_IDENTIFIER,
            rfc5280::ID_CE_CRL_NUMBER
        ]
    );
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn each_revocation_adds_to_the_previous_crl_with_the_next_number() {
    let d = dir("revoke-append");
    let ca = ca(&d);
    let (a, b) = (
        parse_serial("0A0B").unwrap(),
        parse_serial("0c:0d").unwrap(),
    );
    let first = next_crl(&ca, None, &a, SystemTime::now()).unwrap();
    let second = next_crl(&ca, Some(&first.pem), &b, SystemTime::now()).unwrap();
    assert_eq!(second.number, 2);
    assert_eq!(second.serials, ["0A0B", "0C0D"]);
    // The same one again is already there: listed once, a new number.
    let again = next_crl(&ca, Some(&second.pem), &b, SystemTime::now()).unwrap();
    assert!(again.already);
    assert_eq!((again.number, again.serials.len()), (3, 2));
    assert!(verifies(&ca, &decode(&again.pem)));
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_crl_of_another_ca_or_a_bad_serial_is_refused() {
    let d = dir("revoke-other");
    let ca = ca(&d);
    let (c, k) = test_ca_with(&d, 3650, 5);
    let other = Ca::load(&c, &k).unwrap();
    let serial = parse_serial("0A0B").unwrap();
    let theirs = next_crl(&other, None, &serial, SystemTime::now()).unwrap();
    let err = next_crl(&ca, Some(&theirs.pem), &serial, SystemTime::now()).unwrap_err();
    assert!(err.contains("not signed by this CA"), "{err}");
    assert!(next_crl(&ca, Some("not a CRL"), &serial, SystemTime::now()).is_err());
    assert!(parse_serial("xyz").is_err());
    assert!(parse_serial("00").is_err());
    std::fs::remove_dir_all(d).unwrap();
}

const ANCHOR: &str = "arn:aws:rolesanywhere:us-east-1:1:trust-anchor/t";

#[test]
fn the_crl_is_imported_the_first_time_and_updated_after() {
    let aws = FakeAws::default();
    aws.answer(
        "GET /crls",
        200,
        br#"{"crls":[{"crlId":"c-0","name":"installations","trustAnchorArn":"arn:other"}]}"#,
    );
    aws.answer(
        "POST /crls",
        201,
        br#"{"crl":{"crlId":"c-1","name":"installations","enabled":true}}"#,
    );
    let ra = RolesAnywhere::new(account(&aws));
    let done = upload(&ra, "installations", ANCHOR, "pem").unwrap();
    assert_eq!(done, Uploaded::Imported("c-1".into()));

    let aws = FakeAws::default();
    let listed = format!(
        r#"{{"crls":[{{"crlId":"c-1","name":"installations","enabled":false,"trustAnchorArn":"{ANCHOR}"}}]}}"#
    );
    aws.answer("GET /crls", 200, listed.as_bytes());
    let crl = br#"{"crl":{"crlId":"c-1","name":"installations","enabled":true}}"#;
    aws.answer("PATCH /crl/c-1", 200, crl);
    aws.answer("POST /crl/c-1/enable", 200, crl);
    let ra = RolesAnywhere::new(account(&aws));
    let done = upload(&ra, "installations", ANCHOR, "pem").unwrap();
    assert_eq!(done, Uploaded::Updated("c-1".into()));
    let sent: Vec<_> = aws.sent.borrow().iter().map(|r| r.method.clone()).collect();
    assert_eq!(sent, ["GET", "PATCH", "POST"], "a disabled CRL is enabled");
}
