//! Reading the signed index from a fake bucket.

use base64::Engine;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::{EncodePublicKey, LineEnding};
use serde_json::{json, Value};

use super::*;
use aws_client::fake::{bucket, FakeAws};

pub(crate) const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32].into()).unwrap()
}

pub(crate) fn index_key(seed: u8) -> IndexKey {
    let pem = signing_key(seed)
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    IndexKey::from_public_key_pem(&pem).unwrap()
}

fn platform(version: &str) -> Value {
    let module = |name: &str| {
        json!({
            "version": version,
            "images": { "api": format!("1.dkr.ecr.us-east-1.amazonaws.com/quality-data/{name}/api@sha256:{DIGEST}") },
            "package": { "key": format!("packages/{name}/{version}.tar.gz"), "sha256": DIGEST },
        })
    };
    json!({
        "version": version,
        "upgrade_from": ">=1.0.0",
        "rollout": { "percent": 100 },
        "modules": { "quality-data": module("quality-data"), "orchestrator": module("orchestrator") },
        "notes": "what is new",
    })
}

/// The modules declare what they provide and require.
fn with_capabilities(mut v: Value) -> Value {
    v["modules"]["quality-data"]["provides"] = json!(["core"]);
    v["modules"]["orchestrator"]["requires"] = json!(["core", "sql-query"]);
    v
}

pub(crate) fn index(channel: &str, sequence: u64) -> Vec<u8> {
    let value = json!({
        "format": 1,
        "channel": channel,
        "sequence": sequence,
        "published_at": "2026-11-21T00:00:00Z",
        "versions": [platform("1.3.0"), with_capabilities(platform("1.4.0"))],
    });
    serde_json::to_vec_pretty(&value).unwrap()
}

/// A bucket that serves `bytes` as the stable index, signed with `seed`.
pub(crate) fn serving(bytes: &[u8], seed: u8) -> FakeAws {
    let signature: Signature = signing_key(seed).sign(bytes);
    let sig = base64::engine::general_purpose::STANDARD.encode(signature.to_der().as_bytes());
    let aws = FakeAws::default();
    aws.answer("GET /index/stable.json", 200, bytes);
    aws.answer("GET /index/stable.json.sig", 200, sig.as_bytes());
    aws
}

pub(crate) fn reader(name: &str) -> IndexReader {
    let seen = std::env::temp_dir()
        .join(format!("agent-index-{name}-{}", std::process::id()))
        .join("index-seen.json");
    let _ = std::fs::remove_file(&seen);
    IndexReader {
        channel: Channel::Stable,
        key: index_key(1),
        seen,
    }
}

fn cleanup(r: &IndexReader) {
    let _ = std::fs::remove_dir_all(r.seen.parent().unwrap());
}

#[test]
fn a_verified_index_offers_its_newest_version_with_every_module() {
    let r = reader("ok");
    let aws = serving(&index("stable", 7), 1);
    let index = r.read(&bucket(&aws)).unwrap();
    let o = offer(&index, "1.2.0", "inst-0001").unwrap();
    assert_eq!(o.version, "1.4.0");
    assert_eq!(o.notes, "what is new");
    assert_eq!(o.modules["orchestrator"].version, "1.4.0");
    assert!(o.modules["quality-data"].images["api"].ends_with(DIGEST));
    let keys: Vec<_> = o.packages.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "packages/orchestrator/1.4.0.tar.gz",
            "packages/quality-data/1.4.0.tar.gz"
        ]
    );
    assert!(o.packages.iter().all(|p| p.sha256 == DIGEST));
    // What each module provides and requires reaches the engine.
    assert_eq!(o.packages[0].requires, ["core", "sql-query"]);
    assert!(o.packages[0].provides.is_empty());
    assert_eq!(o.packages[1].provides, ["core"]);
    // Its sequence is kept.
    assert_eq!(std::fs::read_to_string(&r.seen).unwrap(), r#"{"stable":7}"#);
    cleanup(&r);
}

#[test]
fn an_index_that_does_not_verify_is_not_trusted() {
    let r = reader("bad-signature");
    // Signed with another key.
    let aws = serving(&index("stable", 7), 2);
    let err = r.read(&bucket(&aws)).unwrap_err();
    assert_eq!(
        err,
        CloudError::Rejected("the index signature does not verify".into())
    );
    assert!(!r.seen.exists(), "nothing of it is kept");
    // Changed after it was signed.
    let mut changed = index("stable", 7);
    let aws = serving(&changed, 1);
    changed[10] ^= 1;
    aws.answers.borrow_mut()[0].1.body = changed;
    assert!(matches!(
        r.read(&bucket(&aws)),
        Err(CloudError::Rejected(_))
    ));
    cleanup(&r);
}

#[test]
fn an_older_index_is_a_replay_and_the_same_one_is_read_again() {
    let r = reader("replay");
    r.read(&bucket(&serving(&index("stable", 7), 1))).unwrap();
    r.read(&bucket(&serving(&index("stable", 7), 1))).unwrap();
    let err = r
        .read(&bucket(&serving(&index("stable", 6), 1)))
        .unwrap_err();
    assert!(
        matches!(&err, CloudError::Rejected(m) if m.contains("older")),
        "{err:?}"
    );
    r.read(&bucket(&serving(&index("stable", 8), 1))).unwrap();
    assert_eq!(std::fs::read_to_string(&r.seen).unwrap(), r#"{"stable":8}"#);
    cleanup(&r);
}

#[test]
fn the_index_of_another_channel_is_refused() {
    let r = reader("channel");
    let err = r
        .read(&bucket(&serving(&index("patch", 7), 1)))
        .unwrap_err();
    assert!(matches!(&err, CloudError::Rejected(m) if m.contains("another channel")));
    cleanup(&r);
}

#[test]
fn a_denied_or_unreachable_bucket_says_so() {
    let r = reader("denied");
    let aws = FakeAws::default();
    aws.answer(
        "GET /index/stable.json",
        403,
        b"<Error><Code>AccessDenied</Code></Error>",
    );
    assert!(matches!(
        r.read(&bucket(&aws)),
        Err(CloudError::Rejected(_))
    ));
    let nobody = FakeAws::default();
    assert!(matches!(
        r.read(&bucket(&nobody)),
        Err(CloudError::Network(_))
    ));
    cleanup(&r);
}

#[test]
fn up_to_date_the_installed_version_is_offered() {
    let r = reader("current");
    let index = r.read(&bucket(&serving(&index("stable", 7), 1))).unwrap();
    let o = offer(&index, "1.4.0", "inst-0001").unwrap();
    assert_eq!(o.version, "1.4.0");
    assert!(o.packages.is_empty() && o.modules.is_empty());
    assert!(offer(&index, "dev", "inst-0001").is_err());
    cleanup(&r);
}

#[test]
fn a_first_installation_takes_the_version_named_or_the_newest_that_reaches_it() {
    let mut index = Index::parse(&index("stable", 7)).unwrap();
    assert_eq!(first(&index, None, "inst-0001").unwrap().version, "1.4.0");
    // Paused, or its rollout reaching no one: skipped unless named.
    index.versions[1].paused = true;
    index.versions[0].rollout.percent = 0;
    let err = first(&index, None, "inst-0001").unwrap_err();
    assert_eq!(
        err,
        CloudError::Rejected("the index has no version for this installation".into())
    );
    let o = first(&index, Some("1.4.0"), "inst-0001").unwrap();
    assert_eq!((o.version.as_str(), o.packages.len()), ("1.4.0", 2));
    assert_eq!(
        first(&index, Some("v1.3.0"), "inst-0001").unwrap().version,
        "1.3.0"
    );
    // `upgrade_from` is not read: nothing is installed.
    index.versions[0].upgrade_from = ">=9.0.0".parse().unwrap();
    assert!(first(&index, Some("1.3.0"), "inst-0001").is_ok());
    assert!(
        matches!(first(&index, Some("2.0.0"), "x"), Err(CloudError::Rejected(m)) if m.contains("no version 2.0.0"))
    );
}
