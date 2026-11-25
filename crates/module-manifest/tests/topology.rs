//! `topology`: a module's machine roles, the services each runs and how
//! its machines are updated (decision 0013).

use module_manifest::{Manifest, ManifestError, Strategy, MAX_ROLES};

fn manifest(roles: &str) -> String {
    format!("name: lake-rustfs\nversion: 0.2.1\ntopology:\n  roles:\n{roles}")
}

fn refused(roles: &str) -> String {
    match Manifest::parse(&manifest(roles)) {
        Err(ManifestError::Invalid(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_module_declares_its_roles() {
    let m = Manifest::parse(&manifest(
        "    - { name: lake, services: [rustfs, gate, admin], strategy: all }\n\
         \x20   - { name: lake-replica, services: [rustfs, gate, admin], strategy: all }\n\
         \x20   - { name: trino-worker, services: [worker], strategy: one }\n",
    ))
    .unwrap();
    let roles = &m.topology.as_ref().unwrap().roles;
    assert_eq!(roles.len(), 3);
    assert_eq!(roles[0].name, "lake");
    assert_eq!(roles[0].services, ["rustfs", "gate", "admin"]);
    assert_eq!(roles[0].strategy, Strategy::All);
    assert_eq!(roles[2].strategy.as_str(), "one");
}

#[test]
fn without_topology_a_module_has_no_roles() {
    let m = Manifest::parse("name: docs\nversion: 0.2.0\n").unwrap();
    assert!(m.topology.is_none());
}

#[test]
fn a_strategy_is_one_or_all() {
    // Decision 0002's words, which the manifest no longer takes.
    for s in ["simultaneous", "rolling", "some"] {
        let text = manifest(&format!(
            "    - {{ name: lake, services: [rustfs], strategy: {s} }}\n"
        ));
        assert!(
            matches!(Manifest::parse(&text), Err(ManifestError::Malformed(_))),
            "{s}"
        );
    }
}

#[test]
fn a_role_names_its_services_and_its_strategy() {
    for role in [
        "    - { name: lake, strategy: all }\n",
        "    - { name: lake, services: [rustfs] }\n",
        "    - { name: lake, services: [rustfs], strategy: all, images: [x] }\n",
    ] {
        assert!(
            matches!(
                Manifest::parse(&manifest(role)),
                Err(ManifestError::Malformed(_))
            ),
            "{role}"
        );
    }
    let why = refused("    - { name: lake, services: [], strategy: all }\n");
    assert!(why.contains("1 to 32 services"), "{why}");
    let many: Vec<String> = (0..33).map(|i| format!("s{i}")).collect();
    let why = refused(&format!(
        "    - {{ name: lake, services: [{}], strategy: all }}\n",
        many.join(", ")
    ));
    assert!(why.contains("1 to 32 services"), "{why}");
}

#[test]
fn names_and_services_are_checked_and_unique() {
    let why = refused("    - { name: Lake, services: [rustfs], strategy: all }\n");
    assert!(why.contains("\"Lake\""), "{why}");
    let why = refused("    - { name: lake, services: [\"rust fs\"], strategy: all }\n");
    assert!(why.contains("not a Compose service"), "{why}");
    let why = refused("    - { name: lake, services: [rustfs, rustfs], strategy: all }\n");
    assert!(why.contains("appears twice"), "{why}");
    let why = refused(
        "    - { name: lake, services: [rustfs], strategy: all }\n\
         \x20   - { name: lake, services: [gate], strategy: one }\n",
    );
    assert!(
        why.contains("topology.roles: \"lake\" appears twice"),
        "{why}"
    );
}

#[test]
fn the_masters_own_roles_are_refused() {
    for name in ["app", "updater"] {
        let why = refused(&format!(
            "    - {{ name: {name}, services: [api], strategy: all }}\n"
        ));
        assert!(why.contains("never joins"), "{why}");
    }
}

#[test]
fn at_most_sixteen_roles() {
    let role = |i: usize| format!("    - {{ name: r{i}, services: [s], strategy: one }}\n");
    let fits: String = (0..MAX_ROLES).map(role).collect();
    assert!(Manifest::parse(&manifest(&fits)).is_ok());
    let over: String = (0..=MAX_ROLES).map(role).collect();
    assert!(refused(&over).contains("at most 16 roles"));
}
