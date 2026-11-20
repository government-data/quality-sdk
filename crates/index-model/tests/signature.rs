//! Nothing of an index is trusted before its signature holds.

mod common;

use common::{bytes, index, index_key, platform, sign};
use index_model::{verify, IndexError, IndexKey};

#[test]
fn a_signed_index_verifies_and_parses() {
    let body = bytes(&index(7, vec![platform("3.1.0", ">=3.0.0", 100)]));
    let parsed = verify(&body, &sign(1, &body), &index_key(1)).unwrap();
    assert_eq!(parsed.sequence, 7);
    assert_eq!(parsed.versions[0].version.to_string(), "3.1.0");
}

#[test]
fn one_changed_byte_breaks_the_signature() {
    let body = bytes(&index(7, vec![platform("3.1.0", ">=3.0.0", 100)]));
    let signature = sign(1, &body);
    let tampered = String::from_utf8(body)
        .unwrap()
        .replace("\"percent\": 100", "\"percent\": 10")
        .into_bytes();
    assert_eq!(
        verify(&tampered, &signature, &index_key(1)).unwrap_err(),
        IndexError::BadSignature
    );
}

#[test]
fn another_key_does_not_verify() {
    let body = bytes(&index(7, vec![platform("3.1.0", ">=3.0.0", 100)]));
    assert_eq!(
        verify(&body, &sign(2, &body), &index_key(1)).unwrap_err(),
        IndexError::BadSignature
    );
}

#[test]
fn a_malformed_signature_is_refused_before_anything_is_read() {
    let body = b"not even json";
    for bad in ["", "%%%", "AAAA"] {
        assert!(
            matches!(
                verify(body, bad, &index_key(1)),
                Err(IndexError::MalformedSignature(_))
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn a_valid_signature_over_a_malformed_index_is_still_refused() {
    let body = b"{\"format\": 1}";
    assert!(matches!(
        verify(body, &sign(1, body), &index_key(1)),
        Err(IndexError::Malformed(_))
    ));
}

#[test]
fn the_key_is_read_from_der_and_refused_when_it_is_not_p256() {
    use p256::pkcs8::EncodePublicKey;
    let der = common::signing_key(1)
        .verifying_key()
        .to_public_key_der()
        .unwrap();
    let key = IndexKey::from_public_key_der(der.as_bytes()).unwrap();
    let body = bytes(&index(1, vec![]));
    verify(&body, &sign(1, &body), &key).unwrap();

    assert!(matches!(
        IndexKey::from_public_key_der(b"garbage"),
        Err(IndexError::BadKey(_))
    ));
    assert!(matches!(
        IndexKey::from_public_key_pem("-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----"),
        Err(IndexError::BadKey(_))
    ));
}

/// Signatures made by another implementation, as KMS makes them: OpenSSL 3.6
/// signing the SHA-256 digest of the index (`MessageType=DIGEST`), DER, with
/// a random nonce and no low-S normalisation (3 of the 8 have a high S). The
/// throwaway private key was deleted; only its public key is kept.
#[test]
fn signatures_made_by_openssl_over_the_digest_verify() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/openssl");
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    let key =
        IndexKey::from_public_key_pem(&String::from_utf8(read("index.pub.pem")).unwrap()).unwrap();
    let body = read("index.json");
    for i in 1..=8 {
        let signature = String::from_utf8(read(&format!("index.json.{i}.sig"))).unwrap();
        verify(&body, &signature, &key).unwrap_or_else(|e| panic!("signature {i}: {e}"));
        let mut tampered = body.clone();
        tampered[body.len() / 2] ^= 1;
        assert_eq!(
            verify(&tampered, &signature, &key).unwrap_err(),
            IndexError::BadSignature
        );
    }
}
