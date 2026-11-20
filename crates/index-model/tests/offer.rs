//! Which version an installation is offered, decided by the agent alone.

mod common;

use common::{bytes, index, platform};
use index_model::{bucket, installation_hash, Index};
use semver::Version;
use serde_json::{json, Value};

fn parsed(versions: Vec<Value>) -> Index {
    Index::parse(&bytes(&index(1, versions))).unwrap()
}

fn offered(index: &Index, installed: &str, installation: &str) -> Option<String> {
    index
        .offer(&Version::parse(installed).unwrap(), installation)
        .map(|v| v.version.to_string())
}

/// An installation whose bucket is `b`, found by trying ids.
fn installation_in(b: u8) -> String {
    (0..10_000)
        .map(|i| format!("inst-{i}"))
        .find(|id| bucket(id) == b)
        .expect("some id lands in every bucket")
}

#[test]
fn the_newest_reachable_version_is_offered() {
    let index = parsed(vec![
        platform("3.1.0", ">=3.0.0", 100),
        platform("3.2.0", ">=3.0.0", 100),
        platform("3.0.5", ">=3.0.0", 100),
    ]);
    assert_eq!(offered(&index, "3.0.0", "inst-1").as_deref(), Some("3.2.0"));
}

#[test]
fn never_an_older_or_the_same_version() {
    let index = parsed(vec![platform("3.1.0", "*", 100)]);
    assert_eq!(offered(&index, "3.1.0", "inst-1"), None);
    assert_eq!(offered(&index, "3.2.0", "inst-1"), None);
}

#[test]
fn upgrade_from_holds_back_installations_too_old_to_jump() {
    let index = parsed(vec![
        platform("3.1.0", ">=3.0.0", 100),
        platform("4.0.0", ">=3.1.0", 100),
    ]);
    assert_eq!(offered(&index, "3.0.0", "inst-1").as_deref(), Some("3.1.0"));
    assert_eq!(offered(&index, "3.1.0", "inst-1").as_deref(), Some("4.0.0"));
    assert_eq!(offered(&index, "2.9.0", "inst-1"), None);
}

#[test]
fn a_paused_version_is_skipped_for_the_one_before_it() {
    let mut newest = platform("3.2.0", "*", 100);
    newest["paused"] = json!(true);
    let index = parsed(vec![platform("3.1.0", "*", 100), newest]);
    assert_eq!(offered(&index, "3.0.0", "inst-1").as_deref(), Some("3.1.0"));
}

#[test]
fn a_percentage_reaches_the_buckets_below_it_and_grows_without_removing() {
    let early = installation_in(3);
    let late = installation_in(42);
    for (percent, early_gets, late_gets) in [(0, false, false), (10, true, false), (43, true, true)]
    {
        let index = parsed(vec![platform("3.1.0", "*", percent)]);
        assert_eq!(
            offered(&index, "3.0.0", &early).is_some(),
            early_gets,
            "{percent} %"
        );
        assert_eq!(
            offered(&index, "3.0.0", &late).is_some(),
            late_gets,
            "{percent} %"
        );
    }
    let everyone = parsed(vec![platform("3.1.0", "*", 100)]);
    assert!(offered(&everyone, "3.0.0", &installation_in(99)).is_some());
}

#[test]
fn pilots_get_it_first_named_only_by_their_hash() {
    let pilot = installation_in(80);
    let mut p = platform("3.1.0", "*", 0);
    p["rollout"]["pilots"] = json!([installation_hash(&pilot)]);
    let index = parsed(vec![p]);
    assert_eq!(offered(&index, "3.0.0", &pilot).as_deref(), Some("3.1.0"));
    assert_eq!(
        offered(
            &index,
            "3.0.0",
            &installation_in(80).replace("inst-", "other-")
        ),
        None
    );
}

#[test]
fn the_bucket_is_stable_and_spread() {
    assert_eq!(bucket("inst-1"), bucket("inst-1"));
    let mut seen = [0u32; 100];
    for i in 0..10_000 {
        seen[usize::from(bucket(&format!("inst-{i}")))] += 1;
    }
    // 100 per bucket on average; none empty or more than doubled.
    assert!(seen.iter().all(|&n| (40..=200).contains(&n)), "{seen:?}");
}

#[test]
fn the_installation_hash_is_the_sha256_of_its_id() {
    assert_eq!(
        installation_hash("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
