use super::*;

/// Invented values only.
const SHARED: &str = "invented-shared-secret-value";

fn manifest(roles: &str) -> Manifest {
    Manifest::parse(&format!(
        "name: engine-trino\nversion: 0.1.0\ntopology:\n  roles:\n{roles}"
    ))
    .unwrap()
}

fn told(name: &str) -> ModuleRole {
    ModuleRole {
        name: name.into(),
        strategy: "one".into(),
        services: vec!["trino".into()],
        images: Vec::new(),
        files: Vec::new(),
        env: BTreeMap::new(),
    }
}

fn read(name: &str) -> Result<Secret, String> {
    match name {
        "trino-shared-secret" => Ok(Secret::new(SHARED.into())),
        "big" => Ok(Secret::new("x".repeat(MAX_VALUE + 1))),
        "nul" => Ok(Secret::new("a\0b".into())),
        _ => Err(format!(
            "/opt/quality-data/engine-trino/secrets/{name}: not found"
        )),
    }
}

#[test]
fn a_told_role_gets_its_declared_secrets_and_no_print_shows_them() {
    let m = manifest(
        "    - { name: trino-worker, services: [trino], strategy: one, secrets: [trino-shared-secret] }\n\
         \x20   - { name: coordinator, services: [trino], strategy: one }\n",
    );
    let body = of(
        "engine-trino",
        &m,
        &[told("trino-worker"), told("coordinator")],
        &read,
    )
    .unwrap();
    let json = serde_json::to_value(&body).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "roles": { "trino-worker": { "trino-shared-secret": SHARED } } })
    );
    assert!(!format!("{body:?}").contains(SHARED));
}

#[test]
fn nothing_is_sent_for_a_module_whose_roles_declare_no_secrets() {
    let m = manifest("    - { name: trino-worker, services: [trino], strategy: one }\n");
    assert!(of("engine-trino", &m, &[told("trino-worker")], &read).is_none());
    let plain = Manifest::parse("name: docs\nversion: 0.2.0\n").unwrap();
    assert!(of("docs", &plain, &[], &read).is_none());
}

#[test]
fn a_role_not_told_or_with_a_secret_that_cannot_go_gets_none() {
    for secrets in ["[trino-shared-secret, missing]", "[big]", "[nul]"] {
        let m = manifest(&format!(
            "    - {{ name: trino-worker, services: [trino], strategy: one, secrets: {secrets} }}\n"
        ));
        let body = of("engine-trino", &m, &[told("trino-worker")], &read).unwrap();
        assert!(body.roles.is_empty(), "{secrets}");
    }
    let m = manifest(
        "    - { name: trino-worker, services: [trino], strategy: one, secrets: [trino-shared-secret] }\n",
    );
    let body = of("engine-trino", &m, &[], &read).unwrap();
    assert!(body.roles.is_empty(), "a role the core was not told");
}
