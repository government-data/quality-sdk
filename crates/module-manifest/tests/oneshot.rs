//! `health.oneshot`: services that run once and end (decision 0002,
//! amended 2026-11-25), the agent that first reads them, and what does not
//! read.

use module_manifest::{Manifest, ONESHOT_AGENT};

fn manifest(health: &str) -> Result<Manifest, module_manifest::ManifestError> {
    Manifest::parse(&format!(
        "name: bi-superset\nversion: 0.1.0\nhealth:\n{health}"
    ))
}

#[test]
fn a_manifest_reads_its_one_shot_services_and_needs_the_agent_that_reads_them() {
    let m = manifest("  services: [superset]\n  oneshot: [superset-init]\n").unwrap();
    assert_eq!(m.health.oneshot, ["superset-init"]);
    assert_eq!(
        m.agent_needed().map(|v| v.to_string()).as_deref(),
        Some(ONESHOT_AGENT)
    );
    let none = manifest("  services: [superset]\n").unwrap();
    assert!(none.health.oneshot.is_empty());
    assert_eq!(none.agent_needed(), None);
}

#[test]
fn a_one_shot_service_is_not_one_that_must_run_or_be_checked() {
    for health in [
        "  services: [superset-init]\n  oneshot: [superset-init]\n",
        "  oneshot: [superset-init]\n  checks: [{ service: superset-init, command: [true] }]\n",
        "  oneshot: [superset-init, superset-init]\n",
        "  oneshot: [-superset-init]\n",
    ] {
        assert!(manifest(health).is_err(), "{health}");
    }
}
