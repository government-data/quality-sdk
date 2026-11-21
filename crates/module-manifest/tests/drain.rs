//! `drain`: where the core asks a module what work runs before an update
//! (decision 0002): optional, a path that starts with `/`.

use module_manifest::{Manifest, ManifestError};

fn manifest(drain: &str) -> String {
    format!("name: orchestrator\nversion: 1.0.0\n{drain}")
}

#[test]
fn a_module_declares_its_drain_path() {
    let m = Manifest::parse(&manifest("drain: { path: /internal/drain }\n")).unwrap();
    assert_eq!(m.drain.unwrap().path, "/internal/drain");
    let m = Manifest::parse(&manifest("drain:\n  path: /api/v1/work-in-progress\n")).unwrap();
    assert_eq!(m.drain.unwrap().path, "/api/v1/work-in-progress");
}

#[test]
fn without_drain_the_module_has_no_work_to_drain() {
    assert_eq!(Manifest::parse(&manifest("")).unwrap().drain, None);
}

#[test]
fn a_drain_path_must_be_absolute_and_carry_no_query() {
    let long = format!("/{}", "a".repeat(255));
    for path in [
        "internal/drain",
        "''",
        "'/a b'",
        "'/drain?horizon_secs=60'",
        "'/drain#x'",
        "https://host/drain",
        long.as_str(),
    ] {
        match Manifest::parse(&manifest(&format!("drain: {{ path: {path} }}\n"))) {
            Err(ManifestError::Invalid(why)) => {
                assert!(why.contains("drain.path"), "{path}: {why}")
            }
            other => panic!("{path}: expected Invalid, got {other:?}"),
        }
    }
}

#[test]
fn drain_takes_a_path_only() {
    for text in [
        "drain: { path: /d, port: 8080 }\n",
        "drain: { service: api }\n",
        "drain: /internal/drain\n",
    ] {
        assert!(
            matches!(
                Manifest::parse(&manifest(text)),
                Err(ManifestError::Malformed(_))
            ),
            "{text}"
        );
    }
}
