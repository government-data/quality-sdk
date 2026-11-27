use super::*;

/// Invented values only.
const SHARED: &str = "invented-shared-secret-value";
const PROVIDED: &str = "invented-provided-value";

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

fn read(owner: &str, name: &str) -> Result<Secret, String> {
    match (owner, name) {
        ("engine-trino", "provided") => Ok(Secret::new(PROVIDED.into())),
        (_, "provided") => Err(format!("{owner}/secrets/provided: not found")),
        (_, name) => read_own(name),
    }
}

fn read_own(name: &str) -> Result<Secret, String> {
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
        &[],
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
    assert!(of("engine-trino", &m, &[told("trino-worker")], &[], &read).is_none());
    let plain = Manifest::parse("name: docs\nversion: 0.2.0\n").unwrap();
    assert!(of("docs", &plain, &[], &[], &read).is_none());
}

#[test]
fn a_role_not_told_or_with_a_secret_that_cannot_go_gets_none() {
    for secrets in ["[trino-shared-secret, missing]", "[big]", "[nul]"] {
        let m = manifest(&format!(
            "    - {{ name: trino-worker, services: [trino], strategy: one, secrets: {secrets} }}\n"
        ));
        let body = of("engine-trino", &m, &[told("trino-worker")], &[], &read).unwrap();
        assert!(body.roles.is_empty(), "{secrets}");
    }
    let m = manifest(
        "    - { name: trino-worker, services: [trino], strategy: one, secrets: [trino-shared-secret] }\n",
    );
    let body = of("engine-trino", &m, &[], &[], &read).unwrap();
    assert!(body.roles.is_empty(), "a role the core was not told");
}

#[test]
fn a_secret_from_a_capability_is_read_from_its_provider() {
    let consumer = Manifest::parse(
        "name: query-trino\nversion: 0.1.0\n\
         requires: { capabilities: [{ name: trino-cluster, version: 484, protocols: [trino] }] }\n\
         topology:\n  roles:\n    - { name: trino-worker, services: [worker], strategy: all, \
         secrets: [{ name: provided, from: trino-cluster }] }\n",
    )
    .unwrap();
    let provider = Manifest::parse(
        "name: engine-trino\nversion: 0.1.0\n\
         provides: { capabilities: [{ name: trino-cluster, version: 484, protocols: [trino] }] }\n",
    )
    .unwrap();
    let body = of(
        "query-trino",
        &consumer,
        &[told("trino-worker")],
        &[consumer.clone(), provider],
        &read,
    )
    .unwrap();
    let json = serde_json::to_value(&body).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "roles": { "trino-worker": { "provided": PROVIDED } } })
    );
    let alone = of(
        "query-trino",
        &consumer,
        &[told("trino-worker")],
        &[],
        &read,
    )
    .unwrap();
    assert!(
        alone.roles.is_empty(),
        "no provider installed: the role gets none"
    );
}
