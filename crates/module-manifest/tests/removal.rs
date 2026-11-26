//! `removal`: where the core asks a module whether another may be removed
//! (decision 0015): optional, a path with the rules of `drain.path`, and
//! the agent that first reads it.

use module_manifest::{Manifest, ManifestError, REMOVAL_AGENT};

fn manifest(removal: &str) -> String {
    format!("name: orchestrator\nversion: 1.0.0\n{removal}")
}

#[test]
fn a_module_declares_its_removal_path_and_needs_the_agent_that_asks_it() {
    let m = Manifest::parse(&manifest("removal: { path: /internal/removal }\n")).unwrap();
    assert_eq!(m.removal.as_ref().unwrap().path, "/internal/removal");
    assert_eq!(
        m.agent_needed().map(|v| v.to_string()).as_deref(),
        Some(REMOVAL_AGENT)
    );
    let none = Manifest::parse(&manifest("")).unwrap();
    assert_eq!(none.removal, None);
    assert_eq!(none.agent_needed(), None);
}

#[test]
fn a_removal_path_has_the_rules_of_a_drain_path() {
    for path in [
        "internal/removal",
        "''",
        "'/a b'",
        "'/removal?module=x'",
        "https://host/removal",
    ] {
        match Manifest::parse(&manifest(&format!("removal: {{ path: {path} }}\n"))) {
            Err(ManifestError::Invalid(why)) => {
                assert!(why.contains("removal.path"), "{path}: {why}")
            }
            other => panic!("{path}: expected Invalid, got {other:?}"),
        }
    }
    for text in ["removal: { path: /r, port: 1 }\n", "removal: /r\n"] {
        assert!(Manifest::parse(&manifest(text)).is_err(), "{text}");
    }
}
