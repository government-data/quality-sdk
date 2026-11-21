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
    let calls = f.calls.borrow().clone();
    let order = [
        "cloud",
        "download_package:1.3.0",
        "pull_images:1.3.0",
        "maintenance:on",
        "back_up",
        "save_package:1.2.0",
        // The old file the new one does not carry is removed.
        "put_package:/packages/1.3.0.tar.gz:remove=assets/app-1.2.0.js",
        "put_version:1.3.0",
        "start:1.3.0",
        "healthy:1.3.0",
        "install_commands",
        "maintenance:off",
        "report_to_cloud:Updated",
    ];
    let mut from = 0;
    for step in order {
        let i = calls[from..]
            .iter()
            .position(|l| l == step)
            .unwrap_or_else(|| panic!("missing {step} (after {from}): {calls:#?}"));
        from += i + 1;
    }
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
