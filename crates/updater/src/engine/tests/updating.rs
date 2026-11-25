//! An update end to end, draining what runs and backing up first.

use super::*;

#[test]
fn a_person_updates_end_to_end() {
    let f = Fake::new();
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);

    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.installed_version, "1.3.0");
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Updated);
    assert_eq!(s.available, None);
    assert!(!f.maintenance.get(), "it left maintenance");
    in_order(
        &f,
        &[
            "cloud",
            "download_package:orchestrator:1.3.0",
            "download_package:quality-data:1.3.0",
            "pull_images:1.3.0",
            "maintenance:on",
            "back_up",
            "save_package:orchestrator:1.2.0",
            "save_package:quality-data:1.2.0",
            // The old file the new one does not carry is removed.
            "put_package:orchestrator:/packages/orchestrator-1.3.0.tar.gz:remove=assets/app-1.2.0.js",
            "put_package:quality-data:/packages/quality-data-1.3.0.tar.gz:remove=assets/app-1.2.0.js",
            "put_version:1.3.0",
            // What each new package asks for, before anything starts.
            "provision:orchestrator",
            "provision:quality-data",
            "start:1.3.0",
            "healthy:1.3.0",
            "install_commands",
            "maintenance:off",
            "report_to_cloud:Updated",
        ],
    );
    // Each phase was written before going on.
    let phases: Vec<_> = f.saved.borrow().iter().map(|s| s.phase.name()).collect();
    for phase in [
        "prepared",
        "draining",
        "backing_up",
        "applying",
        "verifying",
        "idle",
    ] {
        assert!(phases.contains(&phase), "{phase} not saved: {phases:?}");
    }
}

#[test]
fn it_waits_for_what_runs_to_finish() {
    let f = Fake::new();
    f.running.set(3);
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.installed_version, "1.3.0");
    // The web saw what was left while it waited, and since when.
    let saved = f.saved.borrow();
    let progress: Vec<_> = saved.iter().filter_map(|g| g.progress.clone()).collect();
    assert_eq!(
        progress,
        ["3 flows running", "2 flows running", "1 flow running"]
    );
    assert!(saved
        .iter()
        .filter(|g| g.phase.name() == "draining")
        .all(|g| g.phase_since.is_some()));
    // When it ends no stale progress is left.
    assert_eq!(s.progress, None);
}

#[test]
fn if_it_does_not_empty_in_time_it_is_postponed_without_touching_anything() {
    let f = Fake::new();
    // Over 30 min of executions, looking every 10 s.
    f.running.set(1000);
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert!(matches!(s.phase, Phase::Prepared { .. }));
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Postponed);
    assert_eq!(s.installed_version, "1.2.0");
    assert!(!f.did("back_up"));
    assert!(!f.maintenance.get(), "it left maintenance");
}

#[test]
fn if_the_backup_fails_nothing_is_touched() {
    let f = Fake::new().failing("backup");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert!(matches!(s.phase, Phase::Prepared { .. }));
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Failed);
    assert!(!f.did("put_version:1.3.0"));
    assert!(!f.maintenance.get());
}

#[test]
fn every_module_of_the_version_reaches_the_host() {
    let f = Fake::new();
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.installed_version, "1.3.0");
    // Every module with every image, as offered.
    assert_eq!(*f.put.borrow(), [("1.3.0".to_string(), modules("1.3.0"))]);
    // What was installed was kept to go back to.
    let previous = f.saved.borrow().iter().find_map(|g| match &g.phase {
        Phase::Applying { previous, .. } => Some(previous.clone()),
        _ => None,
    });
    assert_eq!(previous.unwrap().modules, modules("1.2.0"));
}

#[test]
fn a_provision_that_fails_goes_back_before_anything_starts() {
    let f = Fake::new().failing("provision:orchestrator");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.installed_version, "1.2.0");
    let last = s.last.as_ref().unwrap();
    assert_eq!(last.kind, OutcomeKind::RolledBack);
    assert!(
        last.detail
            .contains("orchestrator: quality-data does not answer"),
        "{}",
        last.detail
    );
    assert!(!f.did("start:1.3.0"), "the new version did not start");
    assert!(f.did("start:1.2.0"), "the previous one started again");
}

#[test]
fn it_saves_each_module_at_its_own_version() {
    let f = Fake::new();
    let mut installed = modules("1.2.0");
    installed.get_mut("orchestrator").unwrap().version = "0.2.0".into();
    *f.installed.borrow_mut() = Some(Installed {
        version: "1.2.0".into(),
        modules: installed,
    });
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);

    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Updated);
    assert_eq!(s.installed_version, "1.3.0");
    in_order(
        &f,
        &[
            "back_up",
            "save_package:orchestrator:0.2.0",
            "save_package:quality-data:1.2.0",
            "put_version:1.3.0",
        ],
    );
}

/// `offer(version)` with the core module at `core` (decision 0014).
fn offer_with_core(version: &str, core: &str) -> Offer {
    let mut o = offer(version);
    o.modules.get_mut("quality-data").unwrap().version = core.into();
    o
}

#[test]
fn a_platform_version_that_changes_only_another_module_updates() {
    let f = Fake::new();
    // The core stays at the installed 1.2.0; only the orchestrator moves.
    *f.cloud.borrow_mut() = Some(Ok(offer_with_core("1.3.0", "1.2.0")));
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);

    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.installed_version, "1.3.0");
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Updated);
    assert!(s.failed.is_empty(), "{:?}", s.failed);
    assert!(!f.did("report_to_cloud:RolledBack"));
}

#[test]
fn a_platform_version_that_changes_the_core_expects_the_new_core() {
    let f = Fake::new();
    // The core moves to 1.2.1 in platform version 1.3.0.
    *f.cloud.borrow_mut() = Some(Ok(offer_with_core("1.3.0", "1.2.1")));
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);

    assert_eq!(s.installed_version, "1.3.0");
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Updated);
    assert_eq!(*f.core.borrow(), "1.2.1");
}

#[test]
fn a_core_that_answers_another_version_rolls_back() {
    // The images put do not change what the core answers.
    let f = Fake::new().failing("core:stuck");
    *f.cloud.borrow_mut() = Some(Ok(offer_with_core("1.3.0", "1.2.1")));
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);

    assert_eq!(s.installed_version, "1.2.0");
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::RolledBack);
    assert!(s
        .last
        .as_ref()
        .unwrap()
        .detail
        .contains("1.2.0 answers, not 1.2.1"));
    assert_eq!(s.failed, ["1.3.0"]);
}
