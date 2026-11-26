//! The check before `remove` and `purge` (decision 0015) with the fake
//! host and a fake core: a refusal stops both and changes nothing, an
//! answer with none lets them go, and a core that cannot be asked refuses
//! only a module whose required capability's provider checks removals.

use super::super::fake_provision::FakeProvision;
use super::super::purge::purge;
use super::super::remove;
use super::super::tests::Fake;
use crate::api::modules::RemovalRefusal;

/// Invented: an orchestrator that checks removals, an engine that needs
/// it, and a module that needs a provider that does not check.
const ORCHESTRATOR: &str = "provides: { capabilities: [{ name: orchestration, version: 1, \
    protocols: [http] }] }\nremoval: { path: /internal/removal }\n";
const ENGINE: &str =
    "requires: { capabilities: [{ name: orchestration, version: 1, protocols: [http] }] }\n";
const LAKE: &str = "provides: { capabilities: [{ name: lake, version: 1, protocols: [s3] }] }\n";
const DOCS: &str = "requires: { capabilities: [{ name: lake, version: 1, protocols: [s3] }] }\n";

fn fake(features: Option<&[&str]>) -> Fake {
    let mut f = Fake::new(&[
        ("quality-data", ""),
        ("orchestrator", ORCHESTRATOR),
        ("engine-x", ENGINE),
        ("lake-rustfs", LAKE),
        ("docs", DOCS),
    ]);
    if let Some(names) = features {
        f.provision = FakeProvision::with_features(names);
    }
    f
}

fn refusing() -> Fake {
    let f = fake(Some(&["module-removal"]));
    *f.provision.refusals.borrow_mut() = vec![RemovalRefusal {
        module: "orchestrator".into(),
        reason: "flows are on its line: move them first".into(),
        counts: [("flows".to_string(), 2), ("runs".to_string(), 1)].into(),
    }];
    f
}

#[test]
fn a_refusal_stops_remove_and_purge_and_changes_nothing() {
    let f = refusing();
    let err = remove(&f, "engine-x").unwrap_err();
    assert_eq!(
        err,
        "not removed: engine-x: orchestrator refuses: flows are on its line: move them first \
         (flows 2, runs 1)"
    );
    let err = purge(&f, "engine-x", "engine-x").unwrap_err();
    assert!(err.contains("orchestrator refuses"), "{err}");
    assert!(
        f.calls().is_empty(),
        "nothing brought down: {:?}",
        f.calls()
    );
    assert_eq!(
        f.provision.notes(),
        ["removal engine-x", "removal engine-x"]
    );
    assert!(f.provision.progress.borrow().is_empty());
    assert!(f.provision.events.borrow().is_empty(), "nothing audited");
}

#[test]
fn no_refusal_lets_the_module_go() {
    let f = fake(Some(&["module-removal"]));
    remove(&f, "engine-x").unwrap();
    assert!(f.calls().contains(&"down engine-x".to_string()));
    assert_eq!(f.provision.notes()[0], "removal engine-x");
}

#[test]
fn a_core_that_cannot_be_asked_refuses_only_under_a_provider_that_checks() {
    for features in [Some(&[][..]), None] {
        let f = fake(features);
        let err = remove(&f, "engine-x").unwrap_err();
        assert!(
            err.contains("engine-x requires orchestration, whose provider orchestrator checks"),
            "{err}"
        );
        if features.is_some() {
            assert!(
                err.contains("\"module-removal\""),
                "names the feature: {err}"
            );
        }
        assert!(f.calls().is_empty());
        assert!(remove(&f, "docs").is_ok(), "lake-rustfs does not check");
    }
}

#[test]
fn a_core_that_fails_the_check_refuses() {
    let f = fake(Some(&["module-removal"]));
    *f.provision.core_down.borrow_mut() = true;
    let err = remove(&f, "docs").unwrap_err();
    assert!(err.contains("could not be asked"), "{err}");
    assert!(f.calls().is_empty());
}
