//! The adapters' own parts: the state file's check and the package's
//! manifest.

use super::*;

const DOCS: &str = "route: { path: /docs, service: web, port: 8080 }\nauth: forward\n";

#[test]
fn a_prepared_version_goes_back_to_idle_and_an_update_under_way_refuses() {
    use updater::state::{Phase, State, Target};
    let dir = std::env::temp_dir().join(format!("qs-modules-state-{}", std::process::id()));
    let file = dir.join("state.json");
    assert!(no_update_running(&file).is_ok(), "no state yet");
    let target = Target {
        version: "1.5.0".into(),
        modules: Modules::new(),
        packages: Vec::new(),
        notes: String::new(),
    };
    let mut s = State::new("1.4.0");
    s.phase = Phase::Prepared {
        target: target.clone(),
    };
    s.save(&file).unwrap();
    assert!(no_update_running(&file).is_ok());
    assert_eq!(State::load(&file).unwrap().unwrap().phase, Phase::Idle);
    s.phase = Phase::BackingUp {
        target,
        request: None,
    };
    s.save(&file).unwrap();
    let err = no_update_running(&file).unwrap_err();
    assert!(err.contains("backing_up"), "{err}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_manifest_is_read_from_the_package() {
    let dir = std::env::temp_dir().join(format!("qs-modules-pkg-{}", std::process::id()));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("module.yaml"),
        format!("name: docs\nversion: 0.2.0\n{DOCS}"),
    )
    .unwrap();
    std::fs::write(src.join("compose.yml"), "services: {}\n").unwrap();
    let package = dir.join("0.2.0.tar.gz");
    crate::package_archive::pack(&src, &package).unwrap();
    let m = package_manifest(&package, &dir.join("scratch"))
        .unwrap()
        .unwrap();
    assert_eq!(m.route.unwrap().path, "/docs");
    assert!(!dir.join("scratch").exists());
    std::fs::remove_file(src.join("module.yaml")).unwrap();
    crate::package_archive::pack(&src, &package).unwrap();
    assert!(package_manifest(&package, &dir.join("scratch"))
        .unwrap()
        .is_none());
    std::fs::remove_dir_all(dir).unwrap();
}
