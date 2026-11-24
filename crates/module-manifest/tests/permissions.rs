//! `permissions` and `menu.requires` (decision 0009): each rule refused and
//! accepted; the manifests of today parse unchanged.

use module_manifest::{Manifest, ManifestError};

/// Part of the example of decision 0009, point 1 (invented values only).
const BI: &str = r"
name: bi-superset
version: 1.0.0
route: { path: /bi, service: superset, port: 8088 }
menu:
  label: { en: Analytics, es: Análisis }
  path: { en: /bi/, es: /bi/ }
  requires: bi-superset:Author
permissions:
  actions:
    - name: bi-superset:Author
      description: { en: Create charts and dashboards, es: Crear gráficos y paneles }
    - name: bi-superset:Administer
      description: { en: Administer the BI tool, es: Administrar la herramienta de BI }
  policies:
    - name: bi-superset:Authors
      description: { en: BI authors, es: Autores de BI }
      actions: [bi-superset:Author]
    - name: bi-superset:Administrators
      description: { en: BI administrators, es: Administradores de BI }
      actions: [bi-superset:Author, bi-superset:Administer]
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
fn the_example_reads_its_actions_policies_and_menu_condition() {
    let m = Manifest::parse(BI).unwrap();
    let menu = m.menu.as_ref().unwrap();
    assert_eq!(menu.requires.as_deref(), Some("bi-superset:Author"));
    let p = m.permissions.as_ref().unwrap();
    assert_eq!(p.actions[1].name, "bi-superset:Administer");
    assert_eq!(p.actions[0].description.es, "Crear gráficos y paneles");
    assert_eq!(p.policies[1].actions.len(), 2);
}

#[test]
fn a_menu_condition_names_an_own_action_or_a_cores() {
    refused(
        &bi_with("requires: bi-superset:Author", "requires: bi-superset:Edit"),
        "not in permissions.actions",
    );
    refused(
        &bi_with("requires: bi-superset:Author", "requires: author"),
        "not an action",
    );
    refused(
        &bi_with(
            "requires: bi-superset:Author",
            "requires: quality:viewResults",
        ),
        "not an action",
    );
    // An action of the core's catalog: the core checks it.
    Manifest::parse(&bi_with(
        "requires: bi-superset:Author",
        "requires: quality:ViewResults",
    ))
    .unwrap();
    let no_permissions = "name: m\nversion: 1.0.0\nroute: { path: /m, service: s, port: 1 }\n\
        menu: { label: { en: M, es: M }, requires: m:Use }\n";
    refused(no_permissions, "not in permissions.actions");
}

#[test]
fn actions_and_policies_carry_the_modules_prefix() {
    refused(
        &bi_with("- name: bi-superset:Author\n", "- name: other:Author\n"),
        "prefix",
    );
    refused(
        &bi_with("- name: bi-superset:Authors", "- name: query-trino:Authors"),
        "prefix",
    );
    refused(
        &bi_with("- name: bi-superset:Authors", "- name: bi-superset:authors"),
        "<module>:<Name>",
    );
    refused(
        &bi_with(
            "- name: bi-superset:Author\n",
            "- name: bi-superset:Au-thor\n",
        ),
        "<module>:<Name>",
    );
}

#[test]
fn each_action_and_policy_once_described_and_allowing_something() {
    refused(
        &bi_with(
            "- name: bi-superset:Administer",
            "- name: bi-superset:Author",
        ),
        "appears twice",
    );
    refused(
        &bi_with(
            "- name: bi-superset:Administrators",
            "- name: bi-superset:Authors",
        ),
        "appears twice",
    );
    refused(
        &bi_with("actions: [bi-superset:Author]", "actions: []"),
        "no action",
    );
    refused(
        &bi_with(
            "actions: [bi-superset:Author]",
            "actions: [bi-superset:Delete]",
        ),
        "not in permissions.actions",
    );
    Manifest::parse(&bi_with(
        "actions: [bi-superset:Author]",
        "actions: [quality:ViewResults]",
    ))
    .unwrap();
    refused(&bi_with("es: Autores de BI", "es: ' '"), "description");
    let long = "x".repeat(201);
    refused(
        &bi_with("en: BI authors", &format!("en: {long}")),
        "description",
    );
}

#[test]
fn todays_manifests_parse_unchanged() {
    for text in [
        include_str!("fixtures/docs.yaml"),
        include_str!("fixtures/quality-data.yaml"),
        include_str!("fixtures/lake-rustfs.yaml"),
    ] {
        let m = Manifest::parse(text).unwrap();
        assert!(m.permissions.is_none());
        assert!(m.menu.is_none_or(|menu| menu.requires.is_none()));
    }
}
