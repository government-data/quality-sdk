//! Publishing into a store in memory, signed by a test key: no bucket, no
//! KMS.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use index_model::{installation_hash, Channel, IndexKey};
use p256::ecdsa::signature::hazmat::PrehashSigner;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::{EncodePublicKey, LineEnding};

use common::dir;
use sdk_cli::publish::images::Registry;
use sdk_cli::publish::input::{parse, Release};
use sdk_cli::publish::{publish, DigestSigner, Store};

/// The release bucket in memory. `race` is served as the index from the
/// second read on, as if another publication came in between.
#[derive(Default)]
struct Memory {
    objects: RefCell<BTreeMap<String, Vec<u8>>>,
    race: RefCell<Option<(Vec<u8>, Vec<u8>)>>,
    index_reads: Cell<u32>,
}

impl Store for Memory {
    fn get_if_exists(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        if key == "index/stable.json" {
            self.index_reads.set(self.index_reads.get() + 1);
        }
        if let Some((json, sig)) = self.race.borrow().as_ref() {
            if self.index_reads.get() > 1 && key.starts_with("index/stable.json") {
                return Ok(Some(if key.ends_with(".sig") { sig } else { json }.clone()));
            }
        }
        Ok(self.objects.borrow().get(key).cloned())
    }

    fn put(&self, key: &str, body: Vec<u8>, _: &str) -> Result<(), String> {
        self.objects.borrow_mut().insert(key.into(), body);
        Ok(())
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        let o = self.objects.borrow();
        Ok(o.keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
}

/// The installations' registry: every image of [`release`] is there, and
/// the digests in `missing` are not.
#[derive(Default)]
struct Ecr {
    missing: Vec<String>,
    asked: RefCell<Vec<(String, String, String)>>,
}

const REGISTRY: &str = "1.dkr.ecr.us-east-1.amazonaws.com";

impl Registry for Ecr {
    fn host(&self) -> Result<String, String> {
        Ok(REGISTRY.into())
    }

    fn has_image(&self, id: &str, repository: &str, digest: &str) -> Result<bool, String> {
        self.asked
            .borrow_mut()
            .push((id.into(), repository.into(), digest.into()));
        Ok(!self.missing.iter().any(|d| d == digest))
    }
}

struct TestSigner(SigningKey);

impl DigestSigner for TestSigner {
    fn sign_digest(&self, digest: &[u8; 32]) -> Result<Vec<u8>, String> {
        let s: Signature = self.0.sign_prehash(digest).map_err(|e| e.to_string())?;
        Ok(s.to_der().as_bytes().to_vec())
    }
}

fn signer(seed: u8) -> TestSigner {
    TestSigner(SigningKey::from_bytes(&[seed; 32].into()).unwrap())
}

fn key(seed: u8) -> IndexKey {
    let pem = signer(seed)
        .0
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    IndexKey::from_public_key_pem(&pem).unwrap()
}

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Platform version `version` with quality-data at it and a package of
/// `bytes`.
fn release(version: &str, bytes: &[u8]) -> Release {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = dir(&format!("publish-{version}-{n}"));
    std::fs::write(d.join("qd.tar.gz"), bytes).unwrap();
    let text = format!(
        r#"
version = "{version}"
upgrade_from = ">=1.3.0, <{version}"
[rollout]
percent = 0
pilots = ["inst-0001"]
[modules.quality-data]
version = "{version}"
package = "qd.tar.gz"
[modules.quality-data.images]
api = "1.dkr.ecr.us-east-1.amazonaws.com/quality-data/quality-data/api@sha256:{DIGEST}"
"#
    );
    let r = parse(&text, &d).unwrap();
    std::fs::remove_dir_all(d).unwrap();
    r
}

fn at(minute: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 21, 12, minute, 0).unwrap()
}

fn stored_index(m: &Memory) -> index_model::Index {
    let o = m.objects.borrow();
    let sig = String::from_utf8(o["index/stable.json.sig"].clone()).unwrap();
    index_model::verify(&o["index/stable.json"], &sig, &key(1)).unwrap()
}

#[test]
fn the_first_publication_writes_sequence_1_that_an_agent_verifies() {
    let m = Memory::default();
    let done = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.4.0", b"p"),
        at(0),
    )
    .unwrap();
    assert_eq!(done.sequence, 1);
    assert_eq!(done.uploaded, ["packages/quality-data/1.4.0.tar.gz"]);
    assert_eq!(
        m.objects.borrow()["packages/quality-data/1.4.0.tar.gz"],
        b"p"
    );
    let index = stored_index(&m);
    assert_eq!((index.sequence, index.channel), (1, Channel::Stable));
    assert_eq!(index.published_at, "2026-11-21T12:00:00Z");
    // The pilot is offered it; the index names no installation.
    let offered = index.offer(&"1.3.0".parse().unwrap(), "inst-0001").unwrap();
    assert_eq!(offered.version.to_string(), "1.4.0");
    assert_eq!(offered.rollout.pilots, [installation_hash("inst-0001")]);
}

#[test]
fn the_next_publication_adds_a_version_with_the_next_sequence() {
    let m = Memory::default();
    publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.4.0", b"p"),
        at(0),
    )
    .unwrap();
    let done = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.5.0", b"q"),
        at(1),
    )
    .unwrap();
    assert_eq!(done.sequence, 2);
    let versions: Vec<_> = stored_index(&m)
        .versions
        .iter()
        .map(|v| v.version.to_string())
        .collect();
    assert_eq!(versions, ["1.4.0", "1.5.0"]);
    // The same version again is refused, and nothing changes.
    let before = m.objects.borrow().clone();
    let err = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.5.0", b"q"),
        at(2),
    )
    .unwrap_err();
    assert!(err.contains("already in index/stable.json"), "{err}");
    assert_eq!(*m.objects.borrow(), before);
}

#[test]
fn a_package_already_there_is_kept_and_other_bytes_under_its_key_refused() {
    let m = Memory::default();
    m.put("packages/quality-data/1.4.0.tar.gz", b"p".to_vec(), "")
        .unwrap();
    let done = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.4.0", b"p"),
        at(0),
    )
    .unwrap();
    assert_eq!((done.uploaded.len(), done.kept.len()), (0, 1));

    let m = Memory::default();
    m.put("packages/quality-data/1.4.0.tar.gz", b"other".to_vec(), "")
        .unwrap();
    let err = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.4.0", b"p"),
        at(0),
    )
    .unwrap_err();
    assert!(err.contains("already holds another package"), "{err}");
    assert!(!m.objects.borrow().contains_key("index/stable.json"));
}

#[test]
fn a_signature_that_does_not_verify_writes_no_index() {
    let m = Memory::default();
    let err = publish(
        &m,
        &signer(2),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.4.0", b"p"),
        at(0),
    )
    .unwrap_err();
    assert!(err.contains("does not verify"), "{err}");
    assert!(!m.objects.borrow().contains_key("index/stable.json"));
}

#[test]
fn a_published_index_that_does_not_verify_or_an_invalid_version_is_refused() {
    let m = Memory::default();
    publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.4.0", b"p"),
        at(0),
    )
    .unwrap();
    m.objects
        .borrow_mut()
        .get_mut("index/stable.json")
        .unwrap()
        .push(b' ');
    let err = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.5.0", b"q"),
        at(1),
    )
    .unwrap_err();
    assert!(err.contains("does not verify"), "{err}");

    let m = Memory::default();
    let mut r = release("1.4.0", b"p");
    let module = r.version.modules.get_mut("quality-data").unwrap();
    module
        .images
        .insert("web".into(), "quality-data/web:latest".into());
    let err = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        r,
        at(0),
    )
    .unwrap_err();
    assert!(err.contains("not pinned by digest"), "{err}");
    assert!(m.objects.borrow().is_empty(), "nothing is written");
}

#[test]
fn a_publication_that_came_in_between_stops_this_one() {
    let other = Memory::default();
    publish(
        &other,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.4.0", b"p"),
        at(0),
    )
    .unwrap();
    let o = other.objects.borrow();
    let m = Memory::default();
    *m.race.borrow_mut() = Some((
        o["index/stable.json"].clone(),
        o["index/stable.json.sig"].clone(),
    ));
    let err = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        release("1.5.0", b"q"),
        at(1),
    )
    .unwrap_err();
    assert!(
        err.contains("reached sequence 1 while publishing 1"),
        "{err}"
    );
    assert!(!m.objects.borrow().contains_key("index/stable.json"));
}

#[test]
fn every_image_must_be_in_the_installations_registry_before_anything_is_written() {
    let m = Memory::default();
    let ecr = Ecr::default();
    publish(
        &m,
        &signer(1),
        &ecr,
        &key(1),
        Channel::Stable,
        release("1.4.0", b"p"),
        at(0),
    )
    .unwrap();
    assert_eq!(
        ecr.asked.borrow()[0],
        (
            "1".to_string(),
            "quality-data/quality-data/api".to_string(),
            format!("sha256:{DIGEST}")
        )
    );

    let m = Memory::default();
    let missing = Ecr {
        missing: vec![format!("sha256:{DIGEST}")],
        ..Ecr::default()
    };
    let err = publish(
        &m,
        &signer(1),
        &missing,
        &key(1),
        Channel::Stable,
        release("1.4.0", b"p"),
        at(0),
    )
    .unwrap_err();
    assert!(
        err.contains("quality-data/api") && err.contains("not in its repository"),
        "{err}"
    );
    assert!(
        m.objects.borrow().is_empty(),
        "nothing is uploaded or signed"
    );

    let mut r = release("1.4.0", b"p");
    let api = r.version.modules.get_mut("quality-data").unwrap();
    api.images.insert(
        "web".into(),
        format!("2.dkr.ecr.us-east-1.amazonaws.com/quality-data/quality-data/web@sha256:{DIGEST}"),
    );
    let err = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        r,
        at(0),
    )
    .unwrap_err();
    assert!(err.contains(&format!("is not in {REGISTRY}")), "{err}");
    assert!(m.objects.borrow().is_empty());
}

/// A package whose `module.yaml` is `manifest`.
fn package_with(manifest: &str) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    let mut h = tar::Header::new_gnu();
    h.set_size(manifest.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    b.append_data(&mut h, "module.yaml", manifest.as_bytes())
        .unwrap();
    b.into_inner().unwrap().finish().unwrap()
}

/// A report of an invented installation, with its agent and channel.
fn report(m: &Memory, installation: &str, at: &str, agent: &str, channel: Option<&str>) {
    let mut r = serde_json::json!({"format": 1, "installation": installation, "agent": agent,
        "kind": "updated", "version": "1.3.0", "from": "1.2.0", "at": at, "detail": ""});
    if let Some(c) = channel {
        r["channel"] = c.into();
    }
    m.objects.borrow_mut().insert(
        format!("reports/{installation}/{at}-updated-1.3.0.json"),
        serde_json::to_vec(&r).unwrap(),
    );
}

const NEEDS_NEW_AGENT: &str =
    "name: quality-data\nversion: 1.4.0\nsecrets: { generate: [{ name: k }] }\n";

fn publish_with(m: &Memory, manifest: &str, min_agent: Option<&str>) -> Vec<String> {
    let mut r = release("1.4.0", &package_with(manifest));
    r.min_agent = min_agent.map(|v| semver::Version::parse(v).unwrap());
    let done = publish(
        m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        r,
        at(0),
    )
    .unwrap();
    assert_eq!(done.sequence, 1, "published either way");
    done.warnings
}

#[test]
fn a_package_that_needs_a_newer_agent_than_the_channel_has_warns() {
    let m = Memory::default();
    // inst-a's newest report is 0.1.0; inst-b follows another channel.
    report(&m, "inst-a", "20261120T000000Z", "0.2.0", None);
    report(&m, "inst-a", "20261121T000000Z", "0.1.0", None);
    report(&m, "inst-b", "20261121T000000Z", "0.0.9", Some("patch"));
    report(&m, "inst-c", "20261121T000000Z", "0.2.0", Some("stable"));
    let w = publish_with(&m, NEEDS_NEW_AGENT, None);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(
        w[0].contains("needs agent 0.2.0") && w[0].contains("has agent 0.1.0"),
        "{w:?}"
    );
    assert!(w[0].contains("2 installations"), "{w:?}");
}

#[test]
fn an_agent_as_new_as_needed_or_no_need_does_not_warn() {
    let m = Memory::default();
    report(&m, "inst-c", "20261121T000000Z", "0.2.0", Some("stable"));
    assert!(publish_with(&m, NEEDS_NEW_AGENT, None).is_empty());
    let m = Memory::default();
    report(&m, "inst-a", "20261121T000000Z", "0.1.0", None);
    assert!(publish_with(&m, "name: quality-data\nversion: 1.4.0\n", None).is_empty());
    // Nothing reported and no minimum: nothing is known, nothing warns.
    assert!(publish_with(&Memory::default(), NEEDS_NEW_AGENT, None).is_empty());
}

#[test]
fn a_configured_minimum_counts_as_the_oldest_agent() {
    let w = publish_with(&Memory::default(), NEEDS_NEW_AGENT, Some("0.1.0"));
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("min_agent 0.1.0"), "{w:?}");
    assert!(publish_with(&Memory::default(), NEEDS_NEW_AGENT, Some("0.2.0")).is_empty());
}

/// engine-trino and query-trino at 1.5.0, query-trino requiring
/// `trino-cluster` 484 of engine-trino, mirroring Trino at `engine` and
/// `worker`.
fn trino_release(engine: char, worker: char) -> Release {
    let d = dir(&format!("publish-trino-{engine}{worker}"));
    let provider = "name: engine-trino\nversion: 0.1.0\nprovides:\n  capabilities:\n    \
                    - { name: trino-cluster, version: 484, protocols: [trino-http] }\n";
    let consumer = "name: query-trino\nversion: 0.1.0\nrequires:\n  capabilities:\n    \
                    - { name: trino-cluster, version: 484, protocols: [trino-http] }\n";
    std::fs::write(d.join("e.tar.gz"), package_with(provider)).unwrap();
    std::fs::write(d.join("q.tar.gz"), package_with(consumer)).unwrap();
    let digest = |c: char| std::iter::repeat_n(c, 64).collect::<String>();
    let text = format!(
        r#"
version = "1.5.0"
upgrade_from = ">=1.3.0, <1.5.0"
[rollout]
percent = 0
[modules.engine-trino]
version = "0.1.0"
package = "e.tar.gz"
provides = ["trino-cluster"]
[modules.engine-trino.images]
trino = "{REGISTRY}/quality-data/engine-trino/trino@sha256:{}"
[modules.query-trino]
version = "0.1.0"
package = "q.tar.gz"
requires = ["trino-cluster"]
[modules.query-trino.images]
trino = "{REGISTRY}/quality-data/query-trino/trino@sha256:{}"
"#,
        digest(engine),
        digest(worker)
    );
    let r = parse(&text, &d).unwrap();
    std::fs::remove_dir_all(d).unwrap();
    r
}

#[test]
fn a_requirer_mirroring_another_digest_of_its_provider_s_image_is_refused() {
    let m = Memory::default();
    let err = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        trino_release('a', 'b'),
        at(0),
    )
    .unwrap_err();
    assert!(
        err.contains("query-trino requires trino-cluster 484 of engine-trino"),
        "{err}"
    );
    assert!(m.objects.borrow().is_empty(), "nothing written");
    let done = publish(
        &m,
        &signer(1),
        &Ecr::default(),
        &key(1),
        Channel::Stable,
        trino_release('a', 'a'),
        at(1),
    )
    .unwrap();
    assert_eq!(done.sequence, 1);
}
