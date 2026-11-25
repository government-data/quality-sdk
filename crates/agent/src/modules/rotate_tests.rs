//! `qd module rotate` with the fake host and the fake provision: the five
//! steps in order, a crash at each resumes with the same new values, the
//! module's own generated files untouched.

use super::super::fake_provision::FakeProvision;
use super::super::tests::Fake;
use super::super::{add, Progress};
use super::*;

/// The module of `provision_tests.rs`, shortened.
const BI: &str = "requires: { capabilities: [{ name: sql-query, version: 1, protocols: [q] }] }\n\
    route: { path: /bi, service: web, port: 8088 }\nauth: oidc\n\
    oidc: { people: { redirect_paths: [/bi/cb] }, machine: { capabilities: [sql-query] } }\n\
    secrets: { generate: [{ name: secret-key }] }\n\
    data: { databases: [{ name: bi, create: true }] }\n";
const TRINO: &str = "provides: { capabilities: [{ name: sql-query, version: 1, protocols: [q], \
    properties: { token-audience: trino } }] }\n";

fn added() -> Fake {
    let mut f = Fake::new(&[("quality-data", ""), ("query-trino", TRINO)]).with_index("bi", BI);
    f.provision = FakeProvision::with_features(&["module-clients"]);
    add(&f, "bi").unwrap();
    f.provision.notes.borrow_mut().clear();
    f.provision.sent.borrow_mut().clear();
    f
}

fn secret(f: &Fake, name: &str) -> String {
    f.provision.secret_of("bi", name).unwrap()
}

#[test]
fn the_issued_secrets_are_rotated_in_five_steps() {
    let f = added();
    let old_people = secret(&f, "oidc-people-secret");
    let own = secret(&f, "secret-key");
    rotate(&f, "bi").unwrap();
    let new_people = secret(&f, "oidc-people-secret");
    assert_ne!(new_people, old_people);
    assert_eq!(
        secret(&f, "secret-key"),
        own,
        "the module's own file is not rotated"
    );
    let sent = f.provision.sent.borrow();
    assert_eq!(sent.len(), 2);
    let people = |i: usize| sent[i].people.as_ref().unwrap().clone();
    assert_eq!(people(0).secret.expose(), new_people);
    assert!(people(0).keep_previous && sent[0].machine.as_ref().unwrap().keep_previous);
    assert_eq!(people(1).secret.expose(), new_people);
    assert!(!people(1).keep_previous);
    let notes = f.provision.notes();
    let password = secret(&f, "database-password");
    assert_eq!(notes[1], format!("create_database bi {password} bi"));
    assert!(
        notes[2..5].iter().all(|n| n.starts_with("promote_next")),
        "{notes:?}"
    );
    let calls = f.calls();
    assert!(
        calls.ends_with(&[
            "up bi".to_string(),
            "healthy bi".into(),
            "report module_rotated 1.4.0 bi 1.0.0".into()
        ]),
        "{calls:?}"
    );
    assert!(f.provision.progress.borrow().is_empty());
    assert_eq!(
        f.provision.event_names().last().unwrap(),
        "ModuleSecretsRotated succeeded"
    );
}

#[test]
fn a_crash_at_each_step_resumes_with_the_same_new_values() {
    for step in STEPS {
        let f = added();
        *f.provision.crash_at.borrow_mut() = Some(step.into());
        // The crash happens when the step is about to be recorded: the
        // step before it is recorded, and done.
        let err = rotate(&f, "bi").unwrap_err();
        assert!(
            err.contains("qd module rotate bi resumes it"),
            "{step}: {err}"
        );
        let report = f.calls().last().unwrap().clone();
        assert!(
            report.starts_with("report rotate_failed 1.4.0 bi 1.0.0 crash before"),
            "{step}: {report}"
        );
        let staged = f.provision.secret_of("bi", "oidc-people-secret.next");
        rotate(&f, "bi").unwrap();
        if let Some(staged) = staged {
            assert_eq!(
                secret(&f, "oidc-people-secret"),
                staged,
                "{step}: .next reused"
            );
        }
        assert!(f
            .provision
            .secret_of("bi", "oidc-people-secret.next")
            .is_none());
        let last = f.provision.sent.borrow().last().cloned().unwrap();
        assert!(!last.people.unwrap().keep_previous, "{step}");
        assert!(f.provision.progress.borrow().is_empty(), "{step}");
    }
}

#[test]
fn a_stopped_rotation_resumes_from_its_step_without_staging_again() {
    let f = added();
    f.provision
        .ensure_secret(
            "bi",
            "oidc-people-secret.next",
            module_manifest::SecretContent::Random { bytes: 32 },
            0,
        )
        .unwrap();
    let staged = secret(&f, "oidc-people-secret.next");
    f.provision.progress.borrow_mut().insert(
        "bi".into(),
        Progress {
            operation: "rotating".into(),
            version: "1.0.0".into(),
            step: "promote".into(),
        },
    );
    rotate(&f, "bi").unwrap();
    assert_eq!(secret(&f, "oidc-people-secret"), staged);
    // Only promote and finish ran: one body sent, without keep_previous.
    assert_eq!(f.provision.sent.borrow().len(), 1);
}

#[test]
fn what_cannot_be_rotated_is_refused() {
    let f = added();
    assert!(rotate(&f, "docs").unwrap_err().contains("not installed"));
    let f = Fake::new(&[("quality-data", ""), ("docs", "")]);
    assert!(rotate(&f, "docs")
        .unwrap_err()
        .contains("no secret the agent issues"));
    let f = added();
    *f.provision.features.borrow_mut() = Some(crate::host::Features::default());
    assert!(rotate(&f, "bi").unwrap_err().contains("module-clients"));
    let f = added();
    f.provision.progress.borrow_mut().insert(
        "bi".into(),
        Progress {
            operation: "removing".into(),
            version: "1.0.0".into(),
            step: "clients".into(),
        },
    );
    assert!(rotate(&f, "bi").unwrap_err().contains("finish that first"));
}
