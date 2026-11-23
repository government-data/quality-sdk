//! `route`, `auth` and `menu`: where a module is served to people, how they
//! get through and its entry in the apps menu (decision 0002).

use module_manifest::{Auth, Manifest, ManifestError};

fn manifest(rest: &str) -> Result<Manifest, ManifestError> {
    Manifest::parse(&format!("name: docs\nversion: 0.2.0\n{rest}"))
}

fn refused(rest: &str, why: &str) {
    match manifest(rest) {
        Err(ManifestError::Invalid(w)) => assert!(w.contains(why), "{rest}: {w}"),
        other => panic!("{rest}: expected Invalid, got {other:?}"),
    }
}

const ROUTE: &str = "route: { path: /docs, service: web, port: 8080 }\n";

#[test]
fn the_docs_module_declares_its_route_auth_and_menu() {
    let m = manifest(&format!(
        "{ROUTE}auth: forward\nmenu:\n  label: {{ en: User guide, es: Guía de usuario }}\n  \
         path: {{ en: /docs/, es: /docs/es/ }}\n"
    ))
    .unwrap();
    let route = m.route.clone().unwrap();
    assert_eq!(
        (route.path.as_str(), route.service.as_str(), route.port),
        ("/docs", "web", 8080)
    );
    assert_eq!(m.auth, Some(Auth::Forward));
    let menu = m.menu.unwrap();
    assert_eq!(menu.label.es, "Guía de usuario");
    assert_eq!(menu.paths(&route).es, "/docs/es/");
}

#[test]
fn without_them_the_module_is_not_served_to_people() {
    let m = manifest("").unwrap();
    assert_eq!((m.route, m.auth, m.menu), (None, None, None));
    assert_eq!(
        manifest(&format!("{ROUTE}auth: oidc\n")).unwrap().auth,
        Some(Auth::Oidc)
    );
}

#[test]
fn a_menu_without_a_path_opens_the_route() {
    let m = manifest(&format!(
        "{ROUTE}menu: {{ label: {{ en: Guide, es: Guía }} }}\n"
    ))
    .unwrap();
    let paths = m.menu.unwrap().paths(&m.route.unwrap());
    assert_eq!((paths.en.as_str(), paths.es.as_str()), ("/docs/", "/docs/"));
}

#[test]
fn auth_and_menu_need_a_route() {
    refused("auth: forward\n", "need a route");
    refused("menu: { label: { en: Guide, es: Guía } }\n", "need a route");
}

#[test]
fn a_route_is_one_segment_the_core_does_not_use() {
    for path in ["docs", "/", "/Docs", "/docs/", "/a/b", "'/docs?x'"] {
        refused(
            &format!("route: {{ path: {path}, service: web, port: 8080 }}\n"),
            "route.path",
        );
    }
    for path in ["/api", "/login", "/apps", "/auth", "/internal", "/modules"] {
        refused(
            &format!("route: {{ path: {path}, service: web, port: 8080 }}\n"),
            "the core's",
        );
    }
    refused(
        "route: { path: /docs, service: 'a b', port: 8080 }\n",
        "route.service",
    );
    refused(
        "route: { path: /docs, service: web, port: 0 }\n",
        "route.port",
    );
    assert!(manifest("route: { path: /docs, service: web, port: 70000 }\n").is_err());
    assert!(manifest("route: { path: /docs, service: web }\n").is_err());
}

#[test]
fn a_menu_names_both_languages_and_stays_under_the_route() {
    refused(
        &format!("{ROUTE}menu: {{ label: {{ en: Guide, es: ' ' }} }}\n"),
        "menu.label",
    );
    assert!(manifest(&format!("{ROUTE}menu: {{ label: {{ en: Guide }} }}\n")).is_err());
    for (en, es) in [
        ("/docsx/", "/docs/"),
        ("/api/", "/docs/"),
        ("/docs/../api/", "/docs/"),
        ("/docs//x", "/docs/"),
        ("'/docs/?q=1'", "/docs/"),
        ("https://evil.test/docs/", "/docs/"),
    ] {
        refused(
            &format!(
                "{ROUTE}menu: {{ label: {{ en: G, es: G }}, path: {{ en: {en}, es: {es} }} }}\n"
            ),
            "menu.path",
        );
    }
}

#[test]
fn auth_is_forward_or_oidc() {
    assert!(manifest(&format!("{ROUTE}auth: basic\n")).is_err());
    assert!(manifest(&format!("{ROUTE}auth: Forward\n")).is_err());
}
