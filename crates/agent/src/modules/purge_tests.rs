//! `qd module purge` with the fake host and the fake provision: the
//! refusals, the confirmation, the core that must answer, a failed backup
//! that deletes nothing, and a crash at each step that resumes.

use super::super::fake_provision::FakeProvision;
use super::super::tests::Fake;
use super::super::{add, Progress};
use super::*;

const BI: &str = "route: { path: /bi, service: web, port: 8088 }\nauth: oidc\n\
    oidc: { people: { redirect_paths: [/bi/cb] } }\n\
    data: { databases: [{ name: bi, create: true }] }\n";

fn added() -> Fake {
    let mut f = Fake::new(&[("quality-data", "")]).with_index("bi", BI);
    f.provision = FakeProvision::with_features(&["module-clients"]);
    add(&f, "bi").unwrap();
    f.provision.notes.borrow_mut().clear();
    f
}

/// What the purge asked, without what `remove` did first.
fn purge_notes(f: &Fake) -> Vec<String> {
    f.provision
        .notes()
        .into_iter()
        .filter(|n| !n.starts_with("disable_clients") && !n.starts_with("database_login"))
        .collect()
}

#[test]
fn an_installed_module_is_removed_backed_up_then_deleted_in_order() {
    let f = added();
    let warnings = purge(&f, "bi", "bi").unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("backups/purge/bi-")),
        "{warnings:?}"
    );
    assert!(f.calls().contains(&"down bi".to_string()), "removed first");
    assert_eq!(
        purge_notes(&f),
        [
            "purge_backup bi",
            "delete_module bi",
            "drop_database bi bi",
            "purge_files bi"
        ]
    );
    let events = f.provision.event_names();
    assert_eq!(
        events[events.len() - 3..],
        [
            "ModuleRemoved succeeded",
            "ModulePurgeStarted succeeded",
            "ModulePurged succeeded"
        ]
    );
    assert!(f.provision.progress.borrow().is_empty());
    assert_eq!(
        f.calls().last().unwrap(),
        "report module_purged 1.4.0 bi 1.0.0"
    );
    // The core is told the modules again at `delete`, after `remove`'s.
    assert_eq!(f.calls().iter().filter(|c| *c == "tell").count(), 3);
}

#[test]
fn without_the_confirmation_or_for_the_core_nothing_happens() {
    let f = added();
    assert!(purge(&f, "bi", "").unwrap_err().contains("confirm"));
    assert!(purge(&f, "bi", "docs").unwrap_err().contains("confirm"));
    assert!(purge(&f, "quality-data", "quality-data")
        .unwrap_err()
        .contains("the core"));
    assert!(purge(&f, "../x", "../x")
        .unwrap_err()
        .contains("cannot name"));
    assert!(purge(&f, "grafana", "grafana")
        .unwrap_err()
        .contains("nothing to purge"));
    assert!(f.provision.notes().is_empty());
    assert!(!f.calls().contains(&"down bi".to_string()));
}

#[test]
fn a_core_that_does_not_answer_refuses_the_purge_before_anything_is_deleted() {
    let f = added();
    *f.provision.core_down.borrow_mut() = true;
    let err = purge(&f, "bi", "bi").unwrap_err();
    assert!(err.contains("must record it first"), "{err}");
    assert!(purge_notes(&f).is_empty(), "nothing backed up or deleted");
    assert_eq!(f.provision.progress.borrow()["bi"].step, "started");
}

#[test]
fn a_failed_backup_deletes_nothing() {
    let f = added();
    *f.provision.backup_fails.borrow_mut() = true;
    let err = purge(&f, "bi", "bi").unwrap_err();
    assert!(err.contains("stopped at backup"), "{err}");
    assert!(purge_notes(&f).is_empty());
    // `add` is refused while the purge is under way.
    assert!(add(&f, "bi")
        .unwrap_err()
        .contains("purge of bi is under way"));
    *f.provision.backup_fails.borrow_mut() = false;
    purge(&f, "bi", "bi").unwrap();
    assert_eq!(purge_notes(&f).len(), 4);
}

#[test]
fn a_crash_at_each_step_resumes_from_it() {
    for (i, step) in STEPS.iter().enumerate() {
        let f = added();
        *f.provision.crash_at.borrow_mut() = Some((*step).into());
        assert!(purge(&f, "bi", "bi").is_err(), "{step}");
        purge(&f, "bi", "bi").unwrap();
        let notes = purge_notes(&f);
        // Every step ran; only the last one recorded before the crash may
        // have run twice (it may not have ended).
        for what in [
            "purge_backup bi",
            "delete_module bi",
            "drop_database bi bi",
            "purge_files bi",
        ] {
            let n = notes.iter().filter(|n| *n == what).count();
            assert!((1..=2).contains(&n), "{step}: {notes:?}");
        }
        assert!(notes.len() <= 5, "{step}: {notes:?}");
        let started = f
            .provision
            .event_names()
            .iter()
            .filter(|e| e.starts_with("ModulePurgeStarted"))
            .count();
        assert!((1..=2).contains(&started), "{step} ({i})");
        assert!(f.provision.progress.borrow().is_empty(), "{step}");
    }
}

#[test]
fn a_purge_recorded_at_a_step_resumes_there() {
    let f = added();
    f.provision.progress.borrow_mut().insert(
        "bi".into(),
        Progress {
            operation: "purging".into(),
            version: "1.0.0".into(),
            step: "drop".into(),
        },
    );
    purge(&f, "bi", "bi").unwrap();
    assert_eq!(purge_notes(&f), ["drop_database bi bi", "purge_files bi"]);
    assert!(
        !f.calls().contains(&"down bi".to_string()),
        "not removed again"
    );
}
