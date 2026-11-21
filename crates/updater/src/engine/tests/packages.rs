//! One package per module (decision 0003): each one checked, all applied in
//! one step in the order of what they require, and all rolled back together.

use super::*;
use crate::engine::order::apply_order;

/// The offer, with quality-data providing `lake` and the orchestrator
/// requiring it: the name order is the wrong one.
fn offer_with_requires(version: &str) -> Offer {
    let mut o = offer(version);
    for p in &mut o.packages {
        match p.module.as_str() {
            "quality-data" => p.provides = vec!["lake".into()],
            _ => p.requires = vec!["lake".into()],
        }
    }
    o
}

#[test]
fn a_module_is_applied_after_those_providing_what_it_requires() {
    let f = Fake::new();
    *f.cloud.borrow_mut() = Some(Ok(offer_with_requires("1.3.0")));
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.installed_version, "1.3.0");
    in_order(
        &f,
        &[
            "download_package:quality-data:1.3.0",
            "download_package:orchestrator:1.3.0",
            "put_package:quality-data:/packages/quality-data-1.3.0.tar.gz:remove=assets/app-1.2.0.js",
            "put_package:orchestrator:/packages/orchestrator-1.3.0.tar.gz:remove=assets/app-1.2.0.js",
            "put_version:1.3.0",
            "start:1.3.0",
        ],
    );
    // The target kept that order.
    let order: Option<Vec<String>> = f.saved.borrow().iter().find_map(|g| match &g.phase {
        Phase::Prepared { target } => {
            Some(target.packages.iter().map(|p| p.module.clone()).collect())
        }
        _ => None,
    });
    assert_eq!(order.unwrap(), ["quality-data", "orchestrator"]);
}

#[test]
fn modules_that_require_each_other_are_not_prepared() {
    let f = Fake::new();
    let mut o = offer_with_requires("1.3.0");
    for p in &mut o.packages {
        if p.module == "quality-data" {
            p.requires = vec!["sql-query".into()];
        } else {
            p.provides = vec!["sql-query".into()];
        }
    }
    *f.cloud.borrow_mut() = Some(Ok(o));
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.phase, Phase::Idle);
    let last = s.last.as_ref().unwrap();
    assert_eq!(last.kind, OutcomeKind::Failed);
    assert!(
        last.detail.contains("require each other"),
        "{}",
        last.detail
    );
    assert!(!f
        .calls
        .borrow()
        .iter()
        .any(|c| c.starts_with("download_package")));
    assert!(!f.did("maintenance:on"));
}

#[test]
fn a_damaged_package_of_one_module_prepares_nothing() {
    let f = Fake::new().failing("package:quality-data");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Failed);
    assert!(!f.did("pull_images:1.3.0"));
    assert!(!f.did("maintenance:on"));
    assert_eq!(s.installed_version, "1.2.0");
}

#[test]
fn if_one_module_does_not_apply_the_whole_version_goes_back() {
    let f = Fake::new().failing("put:/packages/quality-data-1.3.0.tar.gz");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.installed_version, "1.2.0");
    let last = s.last.as_ref().unwrap();
    assert_eq!(last.kind, OutcomeKind::RolledBack);
    assert!(last.detail.contains("could not apply"), "{}", last.detail);
    assert_eq!(s.failed, vec!["1.3.0".to_string()]);
    // The module already put goes back too, and every module of the
    // previous version is put with its images.
    in_order(
        &f,
        &[
            "put_package:orchestrator:/packages/orchestrator-1.3.0.tar.gz:remove=assets/app-1.2.0.js",
            "put_package:orchestrator:/releases/orchestrator-1.2.0.tar:remove=assets/app-1.3.0.js",
            "put_package:quality-data:/releases/quality-data-1.2.0.tar:remove=assets/app-1.3.0.js",
            "put_version:1.2.0",
            "start:1.2.0",
        ],
    );
    assert!(!f.did("put_version:1.3.0"));
    assert_eq!(*f.put.borrow(), [("1.2.0".to_string(), modules("1.2.0"))]);
}

#[test]
fn the_order_is_by_name_when_nothing_requires_another() {
    let mut a = package("b-module", "1.0.0");
    // What no module of the version provides, or what it provides itself,
    // orders nothing.
    a.requires = vec!["nobody-provides-it".into(), "own".into()];
    a.provides = vec!["own".into()];
    let b = package("a-module", "1.0.0");
    let ordered = apply_order(&[a, b]).unwrap();
    let names: Vec<_> = ordered.iter().map(|p| p.module.as_str()).collect();
    assert_eq!(names, ["a-module", "b-module"]);
}

#[test]
fn two_packages_for_one_module_are_refused() {
    let err = apply_order(&[package("m", "1.0.0"), package("m", "1.0.1")]).unwrap_err();
    assert!(err.contains("two packages for the module m"), "{err}");
}
