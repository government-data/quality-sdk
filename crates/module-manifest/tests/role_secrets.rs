//! `topology.roles[].secrets`: the module's secret files a role's machines
//! get from the core (decision 0013, ruling 44), the agent that first reads
//! them, and what does not read.

use module_manifest::{Manifest, MAX_ROLE_SECRETS, ROLE_SECRETS_AGENT};

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
    assert_eq!(role.secrets, ["trino-shared-secret", "lake-access-key"]);
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
