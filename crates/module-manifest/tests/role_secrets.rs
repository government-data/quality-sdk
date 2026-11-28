//! `topology.roles[].secrets`: the module's secret files a role's machines
//! get from the core (decision 0013, ruling 44), the agent that first reads
//! them, and what does not read.

use module_manifest::{Manifest, RoleSecret, MAX_ROLE_SECRETS, ROLE_SECRETS_AGENT};

fn manifest(secrets: &str) -> Result<Manifest, module_manifest::ManifestError> {
    Manifest::parse(&format!(
        "name: engine-trino\nversion: 0.1.0\ntopology:\n  roles:\n    \
         - {{ name: trino-worker, services: [trino], strategy: one, secrets: {secrets} }}\n"
    ))
}

#[test]
fn a_role_names_its_secrets_and_needs_the_agent_that_hands_them() {
    let m = manifest("[trino-shared-secret, lake-access-key]").unwrap();
    let role = &m.topology.as_ref().unwrap().roles[0];
    let names: Vec<&str> = role.secrets.iter().map(RoleSecret::name).collect();
    assert_eq!(names, ["trino-shared-secret", "lake-access-key"]);
    assert_eq!(
        m.agent_needed().map(|v| v.to_string()).as_deref(),
        Some(ROLE_SECRETS_AGENT)
    );
    let none = manifest("[]").unwrap();
    assert_eq!(
        none.agent_needed().map(|v| v.to_string()).as_deref(),
        Some(module_manifest::ROLES_AGENT)
    );
}

#[test]
fn a_bad_secret_list_is_refused() {
    let many: Vec<String> = (0..=MAX_ROLE_SECRETS).map(|i| format!("s{i}")).collect();
    for secrets in [
        "[key, key]".to_string(),
        "[\"\"]".to_string(),
        "[\"a/b\"]".to_string(),
        "[\"..\"]".to_string(),
        format!("[{}]", "a".repeat(65)),
        format!("[{}]", many.join(", ")),
    ] {
        assert!(manifest(&secrets).is_err(), "{secrets}");
    }
}

fn with_requires(secrets: &str) -> Result<Manifest, module_manifest::ManifestError> {
    Manifest::parse(&format!(
        "name: query-trino\nversion: 0.1.0\n\
         requires: {{ capabilities: [{{ name: trino-cluster, version: 484, protocols: [trino] }}] }}\n\
         topology:\n  roles:\n    \
         - {{ name: trino-worker, services: [worker], strategy: all, secrets: {secrets} }}\n"
    ))
}

#[test]
fn a_role_takes_a_secret_from_a_required_capabilitys_provider() {
    let m = with_requires("[own, { name: trino-shared-secret, from: trino-cluster }]").unwrap();
    let role = &m.topology.as_ref().unwrap().roles[0];
    let found: Vec<(&str, Option<&str>)> =
        role.secrets.iter().map(|s| (s.name(), s.from())).collect();
    assert_eq!(
        found,
        [
            ("own", None),
            ("trino-shared-secret", Some("trino-cluster"))
        ]
    );
    assert_eq!(
        m.agent_needed().map(|v| v.to_string()).as_deref(),
        Some(module_manifest::ROLE_SECRETS_FROM_AGENT)
    );
    for bad in [
        "[{ name: s, from: lake }]",
        "[{ name: s }]",
        "[{ name: s, from: trino-cluster, other: x }]",
        "[{ name: \"a/b\", from: trino-cluster }]",
        "[s, { name: s, from: trino-cluster }]",
    ] {
        assert!(with_requires(bad).is_err(), "{bad}");
    }
}
