//! A critical volume backed up by a replica (decision 0002, as decision
//! 0012 amends it): in sync, it is not dumped; with no replica, it is
//! dumped and the update goes ahead; a replica not in sync, or not
//! knowing, refuses.

use super::*;

fn replica(state: ReplicaState) -> Result<Vec<Replica>, String> {
    Ok(vec![Replica {
        module: "lake-rustfs".into(),
        volume: "rustfs-data".into(),
        state,
    }])
}

fn answers(f: &Fake, answers: Vec<Result<Vec<Replica>, String>>) {
    *f.replicas.borrow_mut() = answers.into();
}

fn reports(f: &Fake) -> Vec<String> {
    f.calls
        .borrow()
        .iter()
        .filter(|c| c.starts_with("report_replica_refused:"))
        .cloned()
        .collect()
}

/// Refused before anything was touched: prepared still, not updated, the
/// outcome `failed` with `detail`, reported once.
fn refused(f: &Fake, s: &State, detail: &str) {
    assert!(matches!(s.phase, Phase::Prepared { .. }), "{:?}", s.phase);
    assert_eq!(s.installed_version, "1.2.0");
    let last = s.last.as_ref().unwrap();
    assert_eq!(last.kind, OutcomeKind::Failed);
    assert_eq!(last.detail, format!("not updated: {detail}"));
    assert!(!f.did("back_up") && !f.did("put_version:1.3.0"));
    assert!(!f.maintenance.get(), "it is not left in maintenance");
    assert_eq!(
        reports(f),
        [format!("report_replica_refused:{}", last.detail)]
    );
    assert!(!f.did("report_to_cloud:Failed"));
}

#[test]
fn with_no_replicated_volume_it_updates_as_before() {
    let f = Fake::new();
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.installed_version, "1.3.0");
    in_order(
        &f,
        &[
            "replicas",
            "maintenance:on",
            "replicas",
            "back_up",
            "start:1.3.0",
        ],
    );
    assert!(reports(&f).is_empty());
}

#[test]
fn a_replica_in_sync_is_the_backup_and_it_updates() {
    let f = Fake::new();
    answers(
        &f,
        vec![replica(ReplicaState::InSync), replica(ReplicaState::InSync)],
    );
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.installed_version, "1.3.0");
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Updated);
    assert!(reports(&f).is_empty());
    assert!(!f.calls.borrow().iter().any(|c| c.starts_with("dump:")));
}

#[test]
fn with_no_replica_the_volume_is_dumped_and_it_updates() {
    let f = Fake::new();
    answers(
        &f,
        vec![
            replica(ReplicaState::NoReplica),
            replica(ReplicaState::NoReplica),
        ],
    );
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.installed_version, "1.3.0");
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Updated);
    in_order(
        &f,
        &[
            "maintenance:on",
            "replicas",
            "back_up",
            "dump:lake-rustfs:rustfs-data",
            "start:1.3.0",
        ],
    );
    assert!(reports(&f).is_empty());
}

#[test]
fn with_no_replica_a_failed_dump_stops_the_update() {
    let f = Fake::new().failing("backup");
    answers(
        &f,
        vec![
            replica(ReplicaState::NoReplica),
            replica(ReplicaState::NoReplica),
        ],
    );
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert!(matches!(s.phase, Phase::Prepared { .. }), "{:?}", s.phase);
    assert_eq!(s.installed_version, "1.2.0");
    let last = s.last.as_ref().unwrap();
    assert_eq!(last.kind, OutcomeKind::Failed);
    assert!(last.detail.starts_with("not updated: the backup failed"));
    assert!(f.did("dump:lake-rustfs:rustfs-data"));
    assert!(!f.did("put_version:1.3.0") && !f.maintenance.get());
    assert!(
        reports(&f).is_empty(),
        "a failed backup is no replica refusal"
    );
}

#[test]
fn only_the_volumes_with_no_replica_are_dumped() {
    let f = Fake::new();
    let mut two = replica(ReplicaState::InSync).unwrap();
    two.push(Replica {
        module: "other".into(),
        volume: "store".into(),
        state: ReplicaState::NoReplica,
    });
    answers(&f, vec![Ok(two.clone()), Ok(two)]);
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.installed_version, "1.3.0");
    assert!(f.did("dump:other:store"));
    assert!(!f.did("dump:lake-rustfs:rustfs-data"));
}

#[test]
fn a_configured_replica_not_in_sync_refuses_before_maintenance() {
    for state in [
        ReplicaState::Behind,
        ReplicaState::Failed,
        ReplicaState::Unhealthy,
        ReplicaState::Unknown,
    ] {
        let f = Fake::new();
        answers(&f, vec![replica(state)]);
        let mut s = State::new("1.2.0");
        run(&f, &mut s, Who::Person);
        let detail = format!(
            "lake-rustfs's volume rustfs-data has no replica in sync ({})",
            state.as_str()
        );
        refused(&f, &s, &detail);
        assert!(!f.did("maintenance:on"), "{state:?}: nothing was drained");
    }
}

#[test]
fn a_replica_that_falls_behind_while_draining_refuses_before_the_backup() {
    let f = Fake::new();
    answers(
        &f,
        vec![replica(ReplicaState::InSync), replica(ReplicaState::Behind)],
    );
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    refused(
        &f,
        &s,
        "lake-rustfs's volume rustfs-data has no replica in sync (behind)",
    );
    in_order(&f, &["maintenance:on", "replicas", "maintenance:off"]);
}

#[test]
fn not_knowing_whether_it_is_in_sync_refuses() {
    let f = Fake::new();
    answers(&f, vec![Err("quality-data does not answer".into())]);
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    refused(
        &f,
        &s,
        "whether the replica is in sync is not known (quality-data does not answer)",
    );
}

#[test]
fn one_replica_not_in_sync_among_several_refuses() {
    let f = Fake::new();
    let mut two = replica(ReplicaState::NoReplica).unwrap();
    two.push(Replica {
        module: "other".into(),
        volume: "store".into(),
        state: ReplicaState::Behind,
    });
    answers(&f, vec![Ok(two)]);
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    refused(
        &f,
        &s,
        "other's volume store has no replica in sync (behind)",
    );
}

#[test]
fn the_service_reports_a_refusal_once_while_it_lasts() {
    let f = Fake::new();
    *f.orders.borrow_mut() = automatic();
    f.at_hour(3);
    let behind = || replica(ReplicaState::Behind);
    answers(&f, vec![behind(), behind()]);
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Service);
    f.wait(Duration::from_secs(60));
    run(&f, &mut s, Who::Service);
    assert!(matches!(s.phase, Phase::Prepared { .. }));
    assert_eq!(reports(&f).len(), 1, "{:#?}", f.calls.borrow());
    // In sync again, it goes ahead.
    f.wait(Duration::from_secs(60));
    run(&f, &mut s, Who::Service);
    assert_eq!(s.installed_version, "1.3.0");
}

#[test]
fn a_replica_state_reads_as_the_core_names_it() {
    for state in [
        ReplicaState::InSync,
        ReplicaState::Behind,
        ReplicaState::Failed,
        ReplicaState::NoReplica,
        ReplicaState::Unhealthy,
        ReplicaState::Unknown,
    ] {
        assert_eq!(ReplicaState::parse(state.as_str()), state);
    }
    assert_eq!(ReplicaState::parse("later"), ReplicaState::Unknown);
}
