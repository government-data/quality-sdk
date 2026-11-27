//! `oidc` (decision 0009): the people's client, its role claim and the
//! machine client, each rule refused and accepted; the manifests of today
//! parse unchanged.

use module_manifest::{Manifest, ManifestError, TokenAlg};

/// Part of the example of decision 0009, point 1 (invented values only).
const BI: &str = r"
name: bi-superset
version: 1.0.0
requires:
  capabilities:
    - { name: sql-query, version: 1, protocols: [trino-client-protocol] }
route: { path: /bi, service: superset, port: 8088 }
auth: oidc
oidc:
  people:
    redirect_paths: [/bi/oauth-authorized/qd]
    logout_paths: [/bi/]
    alg: ES256
    role_claim:
      name: bi_roles
      values:
        viewer: quality:ViewResults
        author: bi-superset:Author
  machine:
    capabilities: [sql-query]
permissions:
  actions:
    - name: bi-superset:Author
      description: { en: Create charts and dashboards, es: Crear gráficos y paneles }
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

fn paths(n: usize) -> String {
    let all: Vec<String> = (0..n).map(|i| format!("/bi/{i}")).collect();
    format!("[{}]", all.join(", "))
}

#[test]
fn the_example_reads_both_clients() {
    let m = Manifest::parse(BI).unwrap();
    let oidc = m.oidc.as_ref().unwrap();
    let people = oidc.people.as_ref().unwrap();
    assert_eq!(people.redirect_paths, ["/bi/oauth-authorized/qd"]);
    assert_eq!(people.logout_paths, ["/bi/"]);
    assert_eq!((people.alg, people.pkce), (TokenAlg::ES256, true));
    let claim = people.role_claim.as_ref().unwrap();
    assert_eq!(claim.name, "bi_roles");
    assert_eq!(claim.values["author"], "bi-superset:Author");
    assert_eq!(oidc.machine.as_ref().unwrap().capabilities, ["sql-query"]);
}

#[test]
fn defaults_are_eddsa_with_pkce_and_no_claim() {
    let m = Manifest::parse(
        "name: m\nversion: 1.0.0\nroute: { path: /m, service: s, port: 1 }\n\
         oidc: { people: { redirect_paths: [/m/cb], pkce: false } }\n",
    )
    .unwrap();
    let people = m.oidc.unwrap().people.unwrap();
    assert_eq!((people.alg, people.pkce), (TokenAlg::EdDSA, false));
    assert!(people.logout_paths.is_empty() && people.role_claim.is_none());
}

#[test]
fn auth_oidc_needs_people_and_people_need_a_route() {
    refused(
        "name: m\nversion: 1.0.0\nroute: { path: /m, service: s, port: 1 }\nauth: oidc\n",
        "auth: oidc needs oidc.people",
    );
    refused(
        "name: m\nversion: 1.0.0\nroute: { path: /m, service: s, port: 1 }\nauth: oidc\n\
         oidc: { machine: { capabilities: [] } }\n",
        "auth: oidc needs oidc.people",
    );
    refused(
        "name: m\nversion: 1.0.0\noidc: { people: { redirect_paths: [/m/cb] } }\n",
        "oidc.people needs a route",
    );
}

#[test]
fn oidc_paths_stay_under_the_route() {
    refused(
        &bi_with("[/bi/oauth-authorized/qd]", "[]"),
        "redirect_paths: none",
    );
    for bad in [
        "/other/cb",
        "/bix/cb",
        "/bi/../admin",
        "/bi/cb?x=1",
        "/bi//cb",
    ] {
        refused(
            &bi_with("/bi/oauth-authorized/qd", bad),
            "not under the route",
        );
        refused(
            &bi_with("[/bi/]", &format!("[{bad}]")),
            "not under the route",
        );
    }
    refused(
        &bi_with("[/bi/oauth-authorized/qd]", &paths(9)),
        "at most 8",
    );
    refused(&bi_with("[/bi/]", &paths(9)), "at most 8");
    Manifest::parse(&bi_with("[/bi/oauth-authorized/qd]", &paths(8))).unwrap();
    Manifest::parse(&bi_with("[/bi/]", &paths(8))).unwrap();
}

#[test]
fn a_role_claim_has_a_new_name_and_short_values() {
    for taken in ["policy", "role", "sub", "email", "auth_time"] {
        refused(
            &bi_with("name: bi_roles", &format!("name: {taken}")),
            "sends it already",
        );
    }
    refused(&bi_with("name: bi_roles", "name: BiRoles"), "[a-z_]");
    refused(&bi_with("name: bi_roles", "name: bi-roles"), "[a-z_]");
    let long = "a".repeat(33);
    refused(
        &bi_with("name: bi_roles", &format!("name: {long}")),
        "[a-z_]",
    );
    refused(&bi_with("viewer:", "'view er':"), "[A-Za-z0-9_-]");
    refused(
        &bi_with("viewer: quality:ViewResults", "viewer: viewresults"),
        "not an action",
    );
    refused(
        &bi_with("author: bi-superset:Author", "author: bi-superset:Edit"),
        "not in permissions.actions",
    );
    let values = |n: usize| -> String {
        (0..n)
            .map(|i| format!("        v{i}: quality:View\n"))
            .collect()
    };
    let one = "        viewer: quality:ViewResults\n        author: bi-superset:Author\n";
    refused(&bi_with(one, &values(17)), "1 to 16");
    Manifest::parse(&bi_with(one, &values(16))).unwrap();
}

#[test]
fn a_machine_client_consumes_what_the_module_requires() {
    refused(
        &bi_with("capabilities: [sql-query]", "capabilities: [lake]"),
        "not in requires.capabilities",
    );
    refused(
        &bi_with("capabilities: [sql-query]", "capabilities: []"),
        "no capabilities and no audiences",
    );
    refused(
        &bi_with(
            "capabilities: [sql-query]",
            "capabilities: [sql-query, sql-query]",
        ),
        "appears twice",
    );
    // A machine client needs no route.
    Manifest::parse(
        "name: m\nversion: 1.0.0\n\
         requires: { capabilities: [{ name: sql-query, version: 1, protocols: [x] }] }\n\
         oidc: { machine: { capabilities: [sql-query] } }\n",
    )
    .unwrap();
}

#[test]
fn an_unknown_key_or_algorithm_is_malformed() {
    for text in [
        bi_with("    alg: ES256\n", "    alg: RS256\n"),
        bi_with("    alg: ES256\n", "    scopes: [openid]\n"),
        bi_with("  machine:\n", "  robot:\n"),
    ] {
        assert!(
            matches!(Manifest::parse(&text), Err(ManifestError::Malformed(_))),
            "{text}"
        );
    }
}

#[test]
fn todays_manifests_parse_unchanged() {
    for text in [
        include_str!("fixtures/docs.yaml"),
        include_str!("fixtures/quality-data.yaml"),
        include_str!("fixtures/lake-rustfs.yaml"),
    ] {
        assert!(Manifest::parse(text).unwrap().oidc.is_none());
    }
}
