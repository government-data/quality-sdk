//! `add` and `remove` of a module that gets secrets, a database and OIDC
//! clients (decision 0009, points 7 and 8), with the fake host and the
//! fake provision: a crash after each step converges, the core not
//! answering stops `add` at the clients, `remove` takes away and `add`
//! afterwards reuses, and an older core refuses.

use super::fake_provision::FakeProvision;
use super::tests::Fake;
use super::*;

/// Invented: a module with every 0009 key, and its provider.
const BI: &str = "requires: { capabilities: [{ name: sql-query, version: 1, protocols: [q] }] }\n\
    route: { path: /bi, service: web, port: 8088 }\nauth: oidc\n\
    menu: { label: { en: Analytics, es: Análisis }, requires: 'bi:Author' }\n\
    oidc:\n  people: { redirect_paths: [/bi/cb], role_claim: { name: bi_roles, values: { author: 'bi:Author' } } }\n  \
    machine: { capabilities: [sql-query] }\n\
    secrets: { uid: 1000, generate: [{ name: secret-key }] }\n\
    data: { databases: [{ name: bi, create: true }] }\n\
    permissions: { actions: [{ name: 'bi:Author', description: { en: Author, es: Autor } }] }\n";
const TRINO: &str = "provides: { capabilities: [{ name: sql-query, version: 1, protocols: [q], \
    properties: { token-audience: trino } }] }\n";
const ALL: [&str; 4] = [
    "module-clients",
    "menu-requires",
    "module-permissions",
    "agent-audit",
];

fn fake() -> Fake {
    let mut f = Fake::new(&[("quality-data", ""), ("query-trino", TRINO)]).with_index("bi", BI);
    f.provision = FakeProvision::with_features(&ALL);
    f
}

#[test]
fn add_provides_secrets_database_and_clients_before_the_module_starts() {
    let f = fake();
    add(&f, "bi").unwrap();
    let p = &f.provision;
    let password = p.secret_of("bi", "database-password").unwrap();
    let people = p.secret_of("bi", "oidc-people-secret").unwrap();
    let machine = p.secret_of("bi", "oidc-machine-secret").unwrap();
    assert!(p.secret_of("bi", "secret-key").is_some());
    assert_eq!(
        p.notes(),
        [
            format!("create_database bi {password} bi"),
            format!("put_clients bi Some({people:?}) Some({machine:?})"),
        ]
    );
    let env: Vec<String> = p.env.borrow()["bi"]
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    assert_eq!(
        env,
        [
            "QD_SECRETS_DIR=/opt/quality-data/bi/secrets",
            "QD_DB_HOST=app.qs-test.internal",
            "QD_DB_PORT=5432",
            "QD_DB_USER=bi",
            "QD_DB_NAME=bi",
            "QD_DB_CA_FILE=/opt/quality-data/bi/ca.crt",
            "QD_OIDC_ISSUER=https://qd.example/auth",
            "QD_OIDC_PEOPLE_CLIENT_ID=bi",
            "QD_OIDC_MACHINE_CLIENT_ID=bi-machine",
        ]
    );
    // No secret in .env.
    assert!(!env
        .iter()
        .any(|l| l.contains(&people) || l.contains(&password)));
    assert!(f.calls().ends_with(&[
        "up bi".to_string(),
        "routes".into(),
        "healthy bi".into(),
        "tell".into()
    ]));
    assert_eq!(p.event_names(), ["ModuleAdded succeeded"]);
    assert!(p.progress.borrow().is_empty());
}

#[test]
fn a_crash_before_each_step_and_add_again_converges_with_the_same_secrets() {
    let whole = fake();
    add(&whole, "bi").unwrap();
    let expected = whole.provision.secrets.borrow().clone();
    for step in [
        "prepared", "secrets", "database", "clients", "env", "started",
    ] {
        let f = fake();
        *f.provision.crash_at.borrow_mut() = Some(step.into());
        let err = add(&f, "bi").unwrap_err();
        assert!(err.contains("qd module add bi resumes it"), "{step}: {err}");
        assert_eq!(f.provision.event_names(), ["ModuleAdded failed"], "{step}");
        if step != "prepared" {
            assert_eq!(f.provision.progress.borrow()["bi"].operation, "adding");
        }
        let made = f.provision.secrets.borrow().clone();
        add(&f, "bi").unwrap();
        // Secrets made before the crash are kept, and nothing else is made
        // than an add without a crash makes.
        for (k, v) in &made {
            assert_eq!(f.provision.secrets.borrow()[k], *v, "{step}");
        }
        let keys: Vec<_> = f.provision.secrets.borrow().keys().cloned().collect();
        assert_eq!(keys, expected.keys().cloned().collect::<Vec<_>>(), "{step}");
        assert!(f.provision.progress.borrow().is_empty(), "{step}");
        assert!(f.calls().ends_with(&["tell".to_string()]), "{step}");
    }
}

#[test]
fn a_core_that_does_not_answer_stops_add_at_the_clients() {
    let f = fake();
    *f.provision.core_down.borrow_mut() = true;
    let err = add(&f, "bi").unwrap_err();
    assert!(err.contains("stopped at clients"), "{err}");
    assert!(!f.calls().iter().any(|c| c.starts_with("up")));
    assert_eq!(f.provision.progress.borrow()["bi"].step, "clients");
    *f.provision.core_down.borrow_mut() = false;
    add(&f, "bi").unwrap();
    assert!(f.calls().contains(&"up bi".to_string()));
}

#[test]
fn remove_disables_and_takes_login_and_add_afterwards_reuses_everything() {
    let f = fake();
    add(&f, "bi").unwrap();
    let secrets = f.provision.secrets.borrow().clone();
    f.provision.notes.borrow_mut().clear();
    remove(&f, "bi").unwrap();
    assert_eq!(
        f.provision.notes(),
        ["disable_clients bi", "database_login bi false"]
    );
    assert!(f.provision.progress.borrow().is_empty());
    add(&f, "bi").unwrap();
    assert_eq!(*f.provision.secrets.borrow(), secrets);
    // The same password and secrets are sent again, which enables.
    let password = &secrets[&("bi".to_string(), "database-password".to_string())];
    assert!(f.provision.notes()[2].contains(password.as_str()));
    assert!(f.provision.notes()[3].starts_with("put_clients bi"));
    assert_eq!(
        f.provision.event_names(),
        [
            "ModuleAdded succeeded",
            "ModuleRemoved succeeded",
            "ModuleAdded succeeded"
        ]
    );
}

#[test]
fn a_core_that_does_not_answer_at_remove_leaves_the_clients_to_disable() {
    let f = fake();
    add(&f, "bi").unwrap();
    *f.provision.core_down.borrow_mut() = true;
    let warnings = remove(&f, "bi").unwrap();
    assert!(
        warnings[0].contains("the service disables them"),
        "{warnings:?}"
    );
    let left = f.provision.progress.borrow()["bi"].clone();
    assert_eq!(
        (left.operation.as_str(), left.step.as_str()),
        ("removing", "clients")
    );
    // An add afterwards enables them again and clears it.
    *f.provision.core_down.borrow_mut() = false;
    add(&f, "bi").unwrap();
    assert!(f.provision.progress.borrow().is_empty());
}

#[test]
fn remove_undoes_an_add_that_stopped() {
    let f = fake();
    *f.provision.core_down.borrow_mut() = true;
    add(&f, "bi").unwrap_err();
    *f.provision.core_down.borrow_mut() = false;
    remove(&f, "bi").unwrap();
    assert!(f.calls().contains(&"down bi".to_string()));
    assert!(f.provision.progress.borrow().is_empty());
}

#[test]
fn an_older_core_refuses_clients_and_permissions_before_anything_changes() {
    for (features, why) in [
        (
            &["menu-requires", "module-permissions"][..],
            "module-clients",
        ),
        (&["module-clients"][..], "module-permissions"),
    ] {
        let mut f = fake();
        f.provision = FakeProvision::with_features(features);
        let err = add(&f, "bi").unwrap_err();
        assert!(err.contains(why), "{err}");
        assert!(!f.calls().iter().any(|c| c.starts_with("pull")));
        assert!(f.provision.secrets.borrow().is_empty());
    }
    let mut f = fake();
    f.provision = FakeProvision::default();
    assert!(add(&f, "bi").unwrap_err().contains("does not answer"));
}

#[test]
fn a_consumed_capability_without_a_token_audience_is_refused() {
    let mut f = Fake::new(&[
        ("quality-data", ""),
        (
            "query-trino",
            "provides: { capabilities: [{ name: sql-query, version: 1, protocols: [q] }] }\n",
        ),
    ])
    .with_index("bi", BI);
    f.provision = FakeProvision::with_features(&ALL);
    let err = add(&f, "bi").unwrap_err();
    assert!(err.contains("declares no token-audience"), "{err}");
}

#[test]
fn a_purge_under_way_refuses_add() {
    let f = fake();
    f.provision.progress.borrow_mut().insert(
        "bi".into(),
        Progress {
            operation: "purging".into(),
            version: "1.0.0".into(),
            step: "backup".into(),
        },
    );
    assert!(add(&f, "bi")
        .unwrap_err()
        .contains("purge of bi is under way"));
}

#[test]
fn a_module_with_secrets_only_needs_no_core_feature() {
    let f = Fake::new(&[("quality-data", "")]).with_index(
        "cache",
        "secrets: { generate: [{ name: key, bytes: 16 }] }\n",
    );
    add(&f, "cache").unwrap();
    assert!(f.provision.secret_of("cache", "key").is_some());
    assert_eq!(
        f.provision.env.borrow()["cache"],
        [(
            "QD_SECRETS_DIR".to_string(),
            "/opt/quality-data/cache/secrets".to_string()
        )]
    );
}
