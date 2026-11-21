//! The rules a manifest keeps: names, capabilities, holds, health and data.

use module_manifest::{Manifest, ManifestError};

fn invalid(text: &str) -> String {
    match Manifest::parse(text) {
        Err(ManifestError::Invalid(why)) => why,
        other => panic!("expected Invalid, got {other:?}"),
    }
}
#[test]
fn names_that_cannot_be_used_are_invalid() {
    assert!(invalid("name: Docs\nversion: 0.1.0\n").contains("module"));
    assert!(invalid("name: ../x\nversion: 0.1.0\n").contains("module"));
    let cap = |c: &str| format!("name: m\nversion: 1.0.0\nprovides: {{ capabilities: [{c}] }}\n");
    assert!(invalid(&cap("{ name: Lake, version: 1, protocols: [s3] }")).contains("capability"));
    assert!(invalid(&cap("{ name: lake, version: 0, protocols: [s3] }")).contains("start at 1"));
    assert!(invalid(&cap("{ name: lake, version: 1, protocols: [] }")).contains("no protocol"));
    assert!(invalid(&cap("{ name: lake, version: 1, protocols: [S3] }")).contains("protocol"));
    assert!(invalid(&cap("{ name: lake, version: 1, protocols: [s3, s3] }")).contains("twice"));
}

#[test]
fn a_capability_twice_or_a_bad_hold_is_invalid() {
    let one = "{ name: lake, version: 1, protocols: [s3] }";
    let two = "{ name: lake, version: 2, protocols: [s3] }";
    let provides =
        |c: &str| format!("name: m\nversion: 1.0.0\nprovides: {{ capabilities: [{c}] }}\n");
    assert!(invalid(&provides(&format!("{one}, {one}"))).contains("twice"));
    Manifest::parse(&provides(&format!("{one}, {two}"))).unwrap();
    let requires =
        |c: &str| format!("name: m\nversion: 1.0.0\nrequires: {{ capabilities: [{c}] }}\n");
    assert!(invalid(&requires(&format!("{one}, {two}"))).contains("twice"));
    for ttl in ["15", "0m", "m", "15d", "-1s"] {
        let c = format!(
            "{{ name: lake, version: 1, protocols: [s3], hold: {{ ttl: '{ttl}', roles: [r] }} }}"
        );
        assert!(invalid(&provides(&c)).contains("hold.ttl"), "{ttl}");
    }
    let c = "{ name: lake, version: 1, protocols: [s3], hold: { ttl: 1h, roles: [] } }";
    assert!(invalid(&provides(c)).contains("hold.roles"));
    let c = "{ name: lake, version: 1, protocols: [s3], hold: { path: holds } }";
    assert!(invalid(&requires(c)).contains("hold.path"));
}

#[test]
fn health_needs_services_commands_and_a_timeout_in_range() {
    let health = |h: &str| format!("name: m\nversion: 1.0.0\nhealth: {h}\n");
    assert!(invalid(&health("{ services: [api, api] }")).contains("twice"));
    assert!(invalid(&health("{ services: ['a b'] }")).contains("service"));
    assert!(invalid(&health("{ checks: [{ service: api, command: [] }] }")).contains("no command"));
    let t = |s: u64| {
        health(&format!(
            "{{ checks: [{{ service: api, command: [x], timeout_secs: {s} }}] }}"
        ))
    };
    assert!(invalid(&t(0)).contains("timeout_secs"));
    assert!(invalid(&t(301)).contains("timeout_secs"));
    Manifest::parse(&t(300)).unwrap();
}

#[test]
fn data_must_stay_inside_its_volume_and_its_module() {
    let data = |d: &str| format!("name: m\nversion: 1.0.0\ndata: {d}\n");
    assert!(invalid(&data("{ databases: [{ name: 'a;b' }] }")).contains("databases"));
    assert!(invalid(&data("{ databases: [{ name: q }, { name: q }] }")).contains("twice"));
    for path in ["data", "/", "/a/../b", "/a/./b"] {
        let v = format!("{{ volumes: [{{ name: v, service: s, path: '{path}' }}] }}");
        assert!(invalid(&data(&v)).contains("absolute path"), "{path}");
    }
    for path in ["/etc/key", "../key", "a/../../key", ""] {
        let k = format!("{{ keys: [{{ path: '{path}' }}] }}");
        assert!(
            invalid(&data(&k)).contains("module's directory"),
            "{path:?}"
        );
    }
    Manifest::parse(&data("{ keys: [{ path: .env }, { path: secrets/a.key }] }")).unwrap();
}
