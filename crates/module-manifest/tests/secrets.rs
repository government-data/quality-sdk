//! `secrets` and `data.databases[].create` (decision 0009): each rule
//! refused and accepted; the manifests of today parse unchanged.

use module_manifest::{Manifest, ManifestError, SecretContent, SECRET_BYTES};

/// Part of the example of decision 0009, point 1 (invented values only).
const BI: &str = r"
name: bi-superset
version: 1.0.0
secrets:
  uid: 1000
  generate:
    - { name: secret-key, bytes: 32, critical: true }
    - { name: valkey-password, critical: false }
data:
  databases:
    - { name: bi_superset, create: true, critical: true }
";

/// `BI` with `from` replaced by `to` (which must be there).
fn bi_with(from: &str, to: &str) -> String {
    assert!(BI.contains(from), "{from:?} is not in the example");
    BI.replacen(from, to, 1)
}

fn refused(text: &str, why: &str) {
    match Manifest::parse(text) {
        Err(ManifestError::Invalid(w)) => assert!(w.contains(why), "{why:?} not in {w:?}"),
        other => panic!("expected Invalid with {why:?}, got {other:?}"),
    }
}

#[test]
fn the_example_reads_its_secrets_and_database() {
    let m = Manifest::parse(BI).unwrap();
    assert_eq!(m.secrets.uid, 1000);
    assert_eq!(m.secrets.generate[0].name, "secret-key");
    assert_eq!(
        m.secrets.generate[1].content(),
        SecretContent::Random {
            bytes: SECRET_BYTES
        }
    );
    assert!(!m.secrets.generate[1].critical);
    assert!(m.data.databases[0].create);
    assert_eq!(m.created_databases().count(), 1);
    assert_eq!(m.database_role().as_deref(), Some("bi_superset"));
}

#[test]
fn defaults_are_root_critical_and_no_database() {
    let m = Manifest::parse(
        "name: m\nversion: 1.0.0\nsecrets: { generate: [{ name: k }] }\n\
         data: { databases: [{ name: other }] }\n",
    )
    .unwrap();
    assert_eq!(m.secrets.uid, 0);
    assert!(m.secrets.generate[0].critical);
    assert!(!m.data.databases[0].create);
    assert_eq!(m.database_role(), None);
}

#[test]
fn generated_secrets_have_plain_unique_names_and_sizes() {
    for issued in [
        "oidc-people-secret",
        "oidc-machine-secret",
        "database-password",
    ] {
        refused(
            &bi_with("name: secret-key", &format!("name: {issued}")),
            "the agent issues",
        );
    }
    refused(
        &bi_with("name: secret-key", "name: Secret_Key"),
        "[a-z0-9-]",
    );
    refused(&bi_with("name: secret-key", "name: ../key"), "[a-z0-9-]");
    refused(&bi_with("name: secret-key", "name: ''"), "[a-z0-9-]");
    let long = "k".repeat(65);
    refused(
        &bi_with("name: secret-key", &format!("name: {long}")),
        "[a-z0-9-]",
    );
    Manifest::parse(&bi_with(
        "name: secret-key",
        &format!("name: {}", &long[1..]),
    ))
    .unwrap();
    refused(
        &bi_with("name: valkey-password", "name: secret-key"),
        "appears twice",
    );
    refused(&bi_with("bytes: 32", "bytes: 15"), "16 to 64");
    refused(&bi_with("bytes: 32", "bytes: 65"), "16 to 64");
    Manifest::parse(&bi_with("bytes: 32", "bytes: 16")).unwrap();
    Manifest::parse(&bi_with("bytes: 32", "bytes: 64")).unwrap();
}

#[test]
fn created_databases_carry_the_modules_name() {
    Manifest::parse(&bi_with("name: bi_superset,", "name: bi_superset_cache,")).unwrap();
    for name in [
        "superset",
        "bi-superset",
        "bi_superset_",
        "bi_supersetx",
        "bi_superset_Cache",
    ] {
        refused(
            &bi_with("name: bi_superset,", &format!("name: {name},")),
            "bi_superset_<suffix>",
        );
    }
    let long = format!("bi_superset_{}", "x".repeat(52));
    refused(
        &bi_with("name: bi_superset,", &format!("name: {long},")),
        "cannot be named",
    );
    let five: String = (0..5)
        .map(|i| format!("    - {{ name: bi_superset_{i}, create: true }}\n"))
        .collect();
    refused(
        &bi_with(
            "    - { name: bi_superset, create: true, critical: true }\n",
            &five,
        ),
        "at most 4",
    );
    let four: String = (0..4)
        .map(|i| format!("    - {{ name: bi_superset_{i}, create: true }}\n"))
        .collect();
    let m = Manifest::parse(&bi_with(
        "    - { name: bi_superset, create: true, critical: true }\n",
        &four,
    ))
    .unwrap();
    assert_eq!(m.database_role().as_deref(), Some("bi_superset"));
    // A database the module does not ask to create keeps today's rule.
    Manifest::parse(&bi_with(
        "name: bi_superset, create: true",
        "name: Other-DB, create: false",
    ))
    .unwrap();
}

#[test]
fn todays_manifests_parse_unchanged() {
    for text in [
        include_str!("fixtures/docs.yaml"),
        include_str!("fixtures/quality-data.yaml"),
        include_str!("fixtures/lake-rustfs.yaml"),
    ] {
        let m = Manifest::parse(text).unwrap();
        assert!(m.secrets.generate.is_empty());
        assert_eq!(m.secrets.uid, 0);
        assert_eq!(m.database_role(), None);
    }
}

#[test]
fn a_manifest_with_these_keys_needs_the_agent_that_reads_them() {
    let needed = module_manifest::MODULE_ADD_AGENT;
    assert_eq!(
        Manifest::parse(BI)
            .unwrap()
            .agent_needed()
            .unwrap()
            .to_string(),
        needed
    );
    for rest in [
        "secrets: { uid: 1000 }\n",
        "route: { path: /m, service: s, port: 1 }\nmenu: { label: { en: M, es: M }, requires: 'quality:View' }\n",
        "permissions: { actions: [] }\n",
        "route: { path: /m, service: s, port: 1 }\noidc: { people: { redirect_paths: [/m/cb] } }\n",
    ] {
        let m = Manifest::parse(&format!("name: m\nversion: 1.0.0\n{rest}")).unwrap();
        assert_eq!(m.agent_needed().unwrap().to_string(), needed, "{rest}");
    }
    for text in [
        include_str!("fixtures/docs.yaml"),
        include_str!("fixtures/quality-data.yaml"),
        include_str!("fixtures/lake-rustfs.yaml"),
    ] {
        assert_eq!(Manifest::parse(text).unwrap().agent_needed(), None);
    }
}
