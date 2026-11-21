//! Going back, restoring the backup, critical, and a cut halfway.

use super::*;

#[test]
fn if_the_new_one_does_not_answer_it_goes_back_and_is_not_retried_by_itself() {
    let f = Fake::new().failing("healthy:1.3.0");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);

    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.installed_version, "1.2.0");
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::RolledBack);
    assert_eq!(s.failed, vec!["1.3.0".to_string()]);
    assert_eq!(*f.up.borrow(), "1.2.0");
    // The previous package comes back and what only the new one carried goes.
    assert!(f.did("put_package:/releases/1.2.0.tar:remove=assets/app-1.3.0.js"));
    assert!(
        !f.did("restore:/backups/quality-data-1"),
        "no restore was needed"
    );
    assert!(f.did("report_to_cloud:RolledBack"));
    assert!(!f.maintenance.get());

    // The service, in the window, does not retry it.
    *f.orders.borrow_mut() = automatic();
    f.at_hour(3);
    f.calls.borrow_mut().clear();
    s.last_check = None;
    run(&f, &mut s, Who::Service);
    assert!(!f.did("maintenance:on"));
    // A person does.
    f.fails.borrow_mut().clear();
    run(&f, &mut s, Who::Person);
    assert_eq!(s.installed_version, "1.3.0");
    assert!(s.failed.is_empty());
}

#[test]
fn if_the_new_one_does_not_even_start_it_goes_back_too() {
    let f = Fake::new().failing("start:1.3.0");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::RolledBack);
    assert_eq!(*f.up.borrow(), "1.2.0");
}

#[test]
fn if_the_previous_one_does_not_answer_either_the_backup_is_restored() {
    let f = Fake::new()
        .failing("healthy:1.3.0")
        .failing("healthy:previous");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Restored);
    assert!(f.did("restore:/backups/quality-data-1"));
    assert_eq!(s.failed, vec!["1.3.0".to_string()]);
    assert!(!f.maintenance.get());
}

#[test]
fn if_not_even_restoring_works_it_is_critical_and_warns() {
    let f = Fake::new()
        .failing("healthy:1.3.0")
        .failing("healthy:previous")
        .failing("restore");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert!(matches!(s.phase, Phase::Critical { .. }), "{:?}", s.phase);
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Critical);
    assert!(f.did("report_to_cloud:Critical"));
    // Critical does nothing by itself.
    f.calls.borrow_mut().clear();
    run(&f, &mut s, Who::Service);
    assert!(f.calls.borrow().is_empty());
}

#[test]
fn a_cut_halfway_goes_on_from_the_saved_phase() {
    // The power went off in each phase: back on, from the last saved one, it
    // ends in the new version or the previous one, never in between.
    let f = Fake::new();
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    let saved = f.saved.borrow().clone();
    for (i, stored) in saved.iter().enumerate() {
        if !stored.phase.in_progress()
            && !matches!(
                stored.phase,
                Phase::Draining { .. } | Phase::BackingUp { .. }
            )
        {
            continue;
        }
        let f = Fake::new();
        // What was applied before the cut is already up.
        if matches!(stored.phase, Phase::Verifying { .. }) {
            *f.up.borrow_mut() = "1.3.0".into();
        }
        let mut s = stored.clone();
        run(&f, &mut s, Who::Service);
        assert_eq!(
            s.phase,
            Phase::Idle,
            "cut at save {i} ({})",
            stored.phase.name()
        );
        assert_eq!(
            s.installed_version,
            "1.3.0",
            "cut at {}",
            stored.phase.name()
        );
        assert!(!f.maintenance.get());
    }
}
