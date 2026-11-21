//! Preparing a version and the policy that decides when it starts.

use super::*;

#[test]
fn it_never_downgrades() {
    let f = Fake::new();
    *f.cloud.borrow_mut() = Some(Ok(offer("1.1.9")));
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.available, None);
    assert!(!f.did("download_package:1.1.9"));
    assert!(is_newer("1.10.0", "1.9.9"));
    assert!(!is_newer("1.2.0", "1.2.0"));
    assert!(
        is_newer("1.2.0", "dev"),
        "a development one takes the published one"
    );
    assert!(!is_newer("dev", "1.2.0"));
}

#[test]
fn up_to_date_it_does_nothing() {
    let f = Fake::new();
    *f.cloud.borrow_mut() = Some(Ok(offer("1.2.0")));
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.last, None);
    assert!(!f.did("maintenance:on"));
}

#[test]
fn without_the_cloud_or_refused_it_warns_and_touches_nothing() {
    for error in [
        CloudError::Network("no connection".into()),
        CloudError::Rejected("the enrolment was revoked".into()),
    ] {
        let f = Fake::new();
        *f.cloud.borrow_mut() = Some(Err(error.clone()));
        let mut s = State::new("1.2.0");
        run(&f, &mut s, Who::Service);
        assert_eq!(s.phase, Phase::Idle);
        assert!(s.warning.is_some());
        assert!(!f.did("maintenance:on"));
    }
}

#[test]
fn a_damaged_package_is_not_prepared() {
    let f = Fake::new().failing("package");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Service);
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Failed);
    assert!(!f.did("maintenance:on"));
}

#[test]
fn without_space_nothing_is_downloaded() {
    let f = Fake::new().failing("space");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Service);
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Failed);
    assert!(!f.did("download_package:1.3.0"));
}

#[test]
fn with_approval_it_prepares_and_waits_for_the_button() {
    let f = Fake::new();
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Service);
    assert!(matches!(s.phase, Phase::Prepared { .. }), "{:?}", s.phase);
    assert_eq!(s.available.as_ref().unwrap().version, "1.3.0");
    assert!(!f.did("maintenance:on"));

    // They press "Update".
    f.orders.borrow_mut().request = Some(Request { id: "p1".into() });
    run(&f, &mut s, Who::Service);
    assert_eq!(s.installed_version, "1.3.0");
    assert_eq!(s.handled_request.as_deref(), Some("p1"));

    // The same request is not handled twice.
    f.calls.borrow_mut().clear();
    *f.cloud.borrow_mut() = Some(Ok(offer("1.4.0")));
    f.wait(CHECK_EVERY);
    run(&f, &mut s, Who::Service);
    assert!(matches!(s.phase, Phase::Prepared { .. }));
    assert!(!f.did("maintenance:on"));
}

#[test]
fn in_manual_mode_it_only_tells() {
    let f = Fake::new();
    f.orders.borrow_mut().policy.mode = Mode::Manual;
    let mut s = State::new("1.2.0");
    // Outside and inside the window: nothing either way.
    f.at_hour(3);
    run(&f, &mut s, Who::Service);
    assert!(matches!(s.phase, Phase::Prepared { .. }));
    assert!(!f.did("maintenance:on"));
}

#[test]
fn in_automatic_mode_it_waits_for_the_window() {
    let f = Fake::new();
    *f.orders.borrow_mut() = automatic();
    let mut s = State::new("1.2.0");
    f.at_hour(14);
    run(&f, &mut s, Who::Service);
    assert!(
        matches!(s.phase, Phase::Prepared { .. }),
        "it prepares during the day"
    );
    assert!(!f.did("maintenance:on"));

    f.at_hour(3);
    run(&f, &mut s, Who::Service);
    assert_eq!(s.installed_version, "1.3.0");
}

#[test]
fn in_automatic_mode_paused_it_does_not_update() {
    let f = Fake::new();
    *f.orders.borrow_mut() = automatic();
    f.orders.borrow_mut().policy.paused = true;
    f.at_hour(3);
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Service);
    assert!(matches!(s.phase, Phase::Prepared { .. }));
}

#[test]
fn in_automatic_mode_it_does_not_start_if_something_is_scheduled_soon() {
    let f = Fake::new();
    *f.orders.borrow_mut() = automatic();
    f.scheduled.set(1);
    f.at_hour(3);
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Service);
    assert!(matches!(s.phase, Phase::Prepared { .. }));
    assert!(!f.did("maintenance:on"));
    // It passed: it starts.
    f.scheduled.set(0);
    run(&f, &mut s, Who::Service);
    assert_eq!(s.installed_version, "1.3.0");
}
