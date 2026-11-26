use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::installed_module::{Features, InstalledModule};

/// A package of files by path.
struct Fake(BTreeMap<String, Vec<u8>>);

impl Package for Fake {
    fn files_under(&self, path: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        let dir = format!("{path}/");
        let found: Vec<_> = self
            .0
            .iter()
            .filter(|(p, _)| p.as_str() == path || p.starts_with(&dir))
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect();
        if found.is_empty() {
            return Err(format!("{path}: not found"));
        }
        Ok(found)
    }
}

fn package(files: &[(&str, &[u8])]) -> Fake {
    Fake(
        files
            .iter()
            .map(|(p, b)| ((*p).to_string(), b.to_vec()))
            .collect(),
    )
}

const MANIFEST: &str = "name: orchestrator\nversion: 2.0.0\n\
provides:\n  events:\n    - { topic: orchestrator.run.finished, version: 1, schema: events/run.json }\n\
consumes:\n  events:\n    - { topic: core.iam.changed, version: 1 }\n";

const SCHEMA: &[u8] =
    br#"{"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object"}"#;

fn manifest(text: &str) -> Manifest {
    Manifest::parse(text).unwrap()
}

#[test]
fn a_module_s_events_carry_each_schema_read_from_its_package() {
    let events = derive(
        "orchestrator",
        &manifest(MANIFEST),
        &package(&[("events/run.json", SCHEMA)]),
    )
    .unwrap();
    let mut m = InstalledModule::new("orchestrator", "2.0.0", None, &Features::default());
    m.events = Some(events);
    let body = serde_json::to_value(&m).unwrap();
    assert_eq!(
        body["events"],
        json!({
            "provides": [{
                "topic": "orchestrator.run.finished",
                "version": 1,
                "schema": {"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object"}
            }],
            "consumes": [{"topic": "core.iam.changed", "version": 1}]
        })
    );
}

#[test]
fn a_module_that_declares_no_events_sends_none() {
    let plain = manifest("name: docs\nversion: 0.2.0\n");
    assert_eq!(derive("docs", &plain, &package(&[])), None);
    let m = InstalledModule::new("docs", "0.2.0", Some(&plain), &Features::default());
    assert!(serde_json::to_value(&m).unwrap().get("events").is_none());
}

#[test]
fn a_module_that_only_consumes_sends_no_schema() {
    let m = manifest(
        "name: bi\nversion: 1.0.0\nconsumes:\n  events:\n    - { topic: core.iam.changed, version: 1 }\n",
    );
    let events = derive("bi", &m, &package(&[])).unwrap();
    assert!(events.provides.is_empty());
    assert_eq!(events.consumes.len(), 1);
}

#[test]
fn a_schema_that_cannot_be_told_leaves_the_module_s_events_out() {
    let big = vec![b' '; MAX_SCHEMA_BYTES + 1];
    for files in [
        vec![],
        vec![("events/run.json", b"[1, 2]".as_slice())],
        vec![("events/run.json", b"{not json".as_slice())],
        vec![("events/run.json", big.as_slice())],
        vec![("events/run.json/a.json", SCHEMA)],
    ] {
        let p = package(&files);
        assert_eq!(derive("orchestrator", &manifest(MANIFEST), &p), None);
    }
}

#[test]
fn adding_a_module_with_events_to_a_core_that_does_not_take_them_warns() {
    use crate::modules::provision::events_warning;
    let m = manifest(MANIFEST);
    let with = Features([Features::MODULE_EVENTS.to_string()].into());
    assert_eq!(events_warning(&m, Ok(with)), None);
    let warned = events_warning(&m, Ok(Features::default())).unwrap();
    assert!(warned.contains("module-events"), "{warned}");
    assert!(events_warning(&m, Err("unreachable".into())).is_some());
}
