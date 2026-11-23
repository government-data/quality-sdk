use std::path::PathBuf;

use module_manifest::Manifest;

use super::*;

fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qs-routes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest(name: &str, rest: &str) -> Option<Manifest> {
    Some(Manifest::parse(&format!("name: {name}\nversion: 1.0.0\n{rest}")).unwrap())
}

const DOCS: &str = "route: { path: /docs, service: web, port: 8080 }\nauth: forward\n";

#[test]
fn a_forward_route_names_the_core_s_chain_and_gate() {
    let m = manifest("docs", DOCS).unwrap();
    let text = render("docs", m.route.as_ref().unwrap(), m.auth);
    for line in [
        "{{ $domain := env \"APP_DOMAIN\" | default \"localhost\" }}",
        "{{ $resolver := \"letsencrypt\" }}{{ if eq (env \"QD_TLS_MODE\") \"file\" }}{{ $resolver = \"\" }}{{ end }}",
        "    module-docs:",
        "      rule: \"Host(`{{ $domain }}`) && (Path(`/docs`) || PathPrefix(`/docs/`))\"",
        "      entryPoints: [websecure]",
        "        certResolver: \"{{ $resolver }}\"",
        "      priority: 50",
        "      service: module-docs",
        "      middlewares: [module-chain@file, module-forward-auth@file]",
        "          - url: \"http://docs-web:8080\"",
    ] {
        assert!(text.lines().any(|l| l == line), "{line}\n---\n{text}");
    }
}

#[test]
fn a_route_without_forward_has_no_gate_of_the_core() {
    for rest in [
        "route: { path: /lake, service: console, port: 9001 }\nauth: oidc\n",
        "route: { path: /lake, service: console, port: 9001 }\n",
    ] {
        let m = manifest("lake-rustfs", rest).unwrap();
        let text = render("lake-rustfs", m.route.as_ref().unwrap(), m.auth);
        assert!(
            text.contains("      middlewares: [module-chain@file]\n"),
            "{text}"
        );
        assert!(text.contains("http://lake-rustfs-console:9001"));
    }
}

#[test]
fn sync_writes_the_installed_routes_and_removes_the_others() {
    let dir = root("sync");
    std::fs::write(dir.join("internal.yml"), "core's").unwrap();
    std::fs::write(dir.join("module-gone.yml"), "old").unwrap();
    let installed = vec![
        ("quality-data".to_string(), manifest("quality-data", "")),
        ("old".to_string(), None),
        ("docs".to_string(), manifest("docs", DOCS)),
    ];
    sync(&dir, &installed).unwrap();
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["internal.yml", "module-docs.yml"]);
    assert!(std::fs::read_to_string(dir.join("module-docs.yml"))
        .unwrap()
        .contains("module-forward-auth@file"));
    assert_eq!(
        std::fs::read_to_string(dir.join("internal.yml")).unwrap(),
        "core's"
    );

    // Unchanged, the file is not written again.
    let before = std::fs::metadata(dir.join("module-docs.yml"))
        .unwrap()
        .modified()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    sync(&dir, &installed).unwrap();
    let after = std::fs::metadata(dir.join("module-docs.yml"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(before, after);

    // The module gone: its route goes.
    sync(&dir, &installed[..1]).unwrap();
    assert!(!dir.join("module-docs.yml").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_route_needs_the_folder_and_none_needs_nothing() {
    let dir = root("missing").join("internal");
    assert!(sync(
        &dir,
        &[("quality-data".into(), manifest("quality-data", ""))]
    )
    .is_ok());
    let err = sync(&dir, &[("docs".into(), manifest("docs", DOCS))]).unwrap_err();
    assert!(err.contains("module-docs.yml"), "{err}");
}

#[test]
fn remove_takes_one_module_s_route_away() {
    let dir = root("remove");
    sync(&dir, &[("docs".into(), manifest("docs", DOCS))]).unwrap();
    remove(&dir, "docs").unwrap();
    assert!(!dir.join("module-docs.yml").exists());
    remove(&dir, "docs").unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
