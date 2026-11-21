//! A module's `provides` and `requires`: optional within format 1, so an
//! index without them still parses, and written back only when present.

mod common;

use common::{bytes, index, platform};
use index_model::{Index, IndexError};
use serde_json::json;

#[test]
fn an_index_without_provides_or_requires_parses_with_none() {
    let parsed = Index::parse(&bytes(&index(3, vec![platform("3.1.0", "*", 10)]))).unwrap();
    let m = &parsed.versions[0].modules["quality-data"];
    assert!(m.provides.is_empty() && m.requires.is_empty());
}

#[test]
fn provides_and_requires_are_read_by_capability_name() {
    let mut p = platform("3.1.0", "*", 10);
    p["modules"]["quality-data"]["requires"] = json!(["lake"]);
    p["modules"]["lake-rustfs"] = common::module("lake-rustfs", "1.0.0");
    p["modules"]["lake-rustfs"]["provides"] = json!(["lake"]);
    let parsed = Index::parse(&bytes(&index(3, vec![p]))).unwrap();
    let modules = &parsed.versions[0].modules;
    assert_eq!(modules["quality-data"].requires, ["lake"]);
    assert!(modules["quality-data"].provides.is_empty());
    assert_eq!(modules["lake-rustfs"].provides, ["lake"]);
}

#[test]
fn an_empty_or_repeated_capability_is_refused() {
    for names in [json!([""]), json!(["lake", "lake"])] {
        let mut p = platform("3.1.0", "*", 10);
        p["modules"]["quality-data"]["requires"] = names;
        match Index::parse(&bytes(&index(3, vec![p]))) {
            Err(IndexError::Invalid(why)) => assert!(why.contains("requires"), "{why}"),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
}

#[test]
fn an_index_written_back_reads_the_same_and_omits_what_is_absent() {
    let mut p = platform("3.1.0", ">=3.0.0, <3.1.0", 10);
    p["modules"]["quality-data"]["provides"] = json!(["lake"]);
    let parsed = Index::parse(&bytes(&index(3, vec![p]))).unwrap();
    let written = serde_json::to_vec(&parsed).unwrap();
    assert_eq!(Index::parse(&written).unwrap(), parsed);
    let value: serde_json::Value = serde_json::from_slice(&written).unwrap();
    let module = &value["versions"][0]["modules"]["quality-data"];
    assert_eq!(module["provides"], json!(["lake"]));
    assert!(module.get("requires").is_none());
    assert_eq!(value["channel"], json!("stable"));
}
