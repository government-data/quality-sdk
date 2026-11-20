//! The rules every index keeps, checked after the signature and before use.

mod common;

use common::{bytes, index, module, platform, DIGEST};
use index_model::{Index, IndexError};
use serde_json::{json, Value};

fn parse(value: &Value) -> Result<Index, IndexError> {
    Index::parse(&bytes(value))
}

fn invalid(value: &Value) -> String {
    match parse(value) {
        Err(IndexError::Invalid(why)) => why,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn a_valid_index_parses_with_its_defaults() {
    let parsed = parse(&index(3, vec![platform("3.1.0", ">=3.0.0", 10)])).unwrap();
    let v = &parsed.versions[0];
    assert!(v.reversible, "reversible unless it says otherwise");
    assert!(!v.urgent && !v.paused);
    assert!(v.rollout.pilots.is_empty());
}

#[test]
fn an_unknown_format_needs_a_newer_agent() {
    let mut v = index(3, vec![]);
    v["format"] = json!(2);
    assert_eq!(parse(&v).unwrap_err(), IndexError::UnsupportedFormat(2));
}

#[test]
fn unknown_fields_are_ignored_within_a_format() {
    let mut v = index(3, vec![platform("3.1.0", ">=3.0.0", 10)]);
    v["comment"] = json!("added by a later publisher");
    parse(&v).unwrap();
}

#[test]
fn an_image_by_tag_is_refused() {
    let mut p = platform("3.1.0", ">=3.0.0", 10);
    p["modules"]["quality-data"]["images"]["api"] = json!("quality-data/api:3.1.0");
    assert!(invalid(&index(3, vec![p])).contains("not pinned by digest"));
    let mut p = platform("3.1.0", ">=3.0.0", 10);
    p["modules"]["quality-data"]["images"]["api"] = json!(format!("@sha256:{DIGEST}"));
    assert!(invalid(&index(3, vec![p])).contains("not pinned by digest"));
}

#[test]
fn a_package_outside_packages_or_with_a_bad_sha_is_refused() {
    for key in [
        "index/stable.json",
        "packages/../index/stable.json",
        "packages//x",
    ] {
        let mut p = platform("3.1.0", ">=3.0.0", 10);
        p["modules"]["quality-data"]["package"]["key"] = json!(key);
        assert!(invalid(&index(3, vec![p])).contains("package key"), "{key}");
    }
    let mut p = platform("3.1.0", ">=3.0.0", 10);
    p["modules"]["quality-data"]["package"]["sha256"] = json!(DIGEST.to_uppercase());
    assert!(invalid(&index(3, vec![p])).contains("SHA-256"));
}

#[test]
fn a_version_twice_no_modules_or_a_rollout_over_100_is_refused() {
    let twice = index(
        3,
        vec![platform("3.1.0", "*", 10), platform("3.1.0", "*", 20)],
    );
    assert!(invalid(&twice).contains("listed twice"));

    let mut empty = platform("3.1.0", "*", 10);
    empty["modules"] = json!({});
    assert!(invalid(&index(3, vec![empty])).contains("no modules"));

    let mut no_images = platform("3.1.0", "*", 10);
    no_images["modules"]["quality-data"] = module("quality-data", "3.1.0");
    no_images["modules"]["quality-data"]["images"] = json!({});
    assert!(invalid(&index(3, vec![no_images])).contains("no images"));

    assert!(invalid(&index(3, vec![platform("3.1.0", "*", 101)])).contains("over 100"));
}

#[test]
fn a_pilot_must_be_a_hash_not_an_id() {
    let mut p = platform("3.1.0", "*", 0);
    p["rollout"]["pilots"] = json!(["inst-1"]);
    assert!(invalid(&index(3, vec![p])).contains("is not a SHA-256"));
}

#[test]
fn an_older_sequence_is_a_replay_and_the_same_one_is_not() {
    let parsed = parse(&index(5, vec![])).unwrap();
    parsed.check_not_older(None).unwrap();
    parsed.check_not_older(Some(4)).unwrap();
    parsed.check_not_older(Some(5)).unwrap();
    assert_eq!(
        parsed.check_not_older(Some(6)).unwrap_err(),
        IndexError::Replay { seen: 6, got: 5 }
    );
}

#[test]
fn not_json_or_a_wrong_type_is_malformed() {
    assert!(matches!(Index::parse(b"[]"), Err(IndexError::Malformed(_))));
    let mut v = index(3, vec![platform("3.1.0", "*", 10)]);
    v["versions"][0]["version"] = json!("three");
    assert!(matches!(parse(&v), Err(IndexError::Malformed(_))));
}
