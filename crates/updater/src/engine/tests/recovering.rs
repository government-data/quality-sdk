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
    // Every previous package comes back and what only the new one carried
    // goes.
    for module in ["orchestrator", "quality-data"] {
        assert!(f.did(&format!(
            "put_package:{module}:/releases/{module}-1.2.0.tar:remove=assets/app-1.3.0.js"
        )));
    }
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
fn a_restore_that_fails_starts_nothing_and_is_reported() {
    let f = Fake::new()
        .failing("healthy:1.3.0")
        .failing("healthy:previous")
        .failing("restore");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    let calls = f.calls.borrow().clone();
    let restore = calls
        .iter()
        .position(|c| c == "restore:/backups/quality-data-1")
        .unwrap();
    // No module is started in a mixed state: a person intervenes.
    assert!(
        !calls[restore..].iter().any(|c| c.starts_with("start:")),
        "{calls:#?}"
    );
    let last = s.last.as_ref().unwrap();
    assert_eq!(last.kind, OutcomeKind::Critical);
    assert!(last.detail.contains("pg_restore failed"), "{}", last.detail);
    assert!(f.did("report_to_cloud:Critical"));
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

#[test]
fn going_back_puts_every_module_of_the_previous_version() {
    let f = Fake::new().failing("healthy:1.3.0");
    let mut s = State::new("1.2.0");
    run(&f, &mut s, Who::Person);
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::RolledBack);
    assert_eq!(
        *f.put.borrow(),
        [
            ("1.3.0".to_string(), modules("1.3.0")),
            ("1.2.0".to_string(), modules("1.2.0")),
        ]
    );
}

#[test]
fn an_update_the_older_engine_left_halfway_goes_on() {
    // Cut while applying, with a state file of the engine before modules.
    let older = serde_json::json!({
        "phase": "applying",
        "target": {
            "version": "1.3.0",
            "api": "r/api:1.3.0@sha256:a",
            "web": "r/web:1.3.0@sha256:b",
            "package": "/packages/1.3.0.tar.gz"
        },
        "previous": {
            "version": "1.2.0",
            "api": "r/api:1.2.0@sha256:c",
            "web": "r/web:1.2.0@sha256:d",
            "package": "/releases/1.2.0.tar",
            "manifest": ["compose.yml", "assets/app-1.2.0.js"]
        },
        "backup": "/backups/quality-data-1",
        "installed_version": "1.2.0"
    });
    let legacy = |version: &str, api: &str, web: &str| {
        let images = [("api", api), ("web", web)]
            .map(|(n, i)| (n.to_string(), i.to_string()))
            .into();
        let module = Module {
            version: version.into(),
            images,
        };
        Modules::from([("quality-data".to_string(), module)])
    };

    // It finishes in the new version, with the images the file named.
    let f = Fake::new();
    let mut s: State = serde_json::from_value(older.clone()).unwrap();
    run(&f, &mut s, Who::Service);
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.installed_version, "1.3.0");
    let new = legacy("1.3.0", "r/api:1.3.0@sha256:a", "r/web:1.3.0@sha256:b");
    assert_eq!(*f.put.borrow(), [("1.3.0".to_string(), new)]);

    // Or, if the new one does not answer, it goes back to the images the
    // file kept.
    let f = Fake::new().failing("healthy:1.3.0");
    let mut s: State = serde_json::from_value(older).unwrap();
    run(&f, &mut s, Who::Service);
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::RolledBack);
    let old = legacy("1.2.0", "r/api:1.2.0@sha256:c", "r/web:1.2.0@sha256:d");
    assert_eq!(f.put.borrow().last(), Some(&("1.2.0".to_string(), old)));
}
