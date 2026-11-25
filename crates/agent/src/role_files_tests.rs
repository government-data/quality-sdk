use super::*;

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("role-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("compose/inner")).unwrap();
    std::fs::write(d.join("compose/a.sh"), b"a").unwrap();
    std::fs::write(d.join("compose/inner/b.conf"), b"bb").unwrap();
    std::fs::write(d.join("module.yaml"), b"m").unwrap();
    d
}

#[test]
fn only_sources_in_the_package_are_files() {
    let volumes: Vec<serde_json::Value> = serde_json::from_str(
        r#"["data:/data", "./x.sh:/x:ro", "./d/:/d", ".:/all", "/etc/hosts:/h",
            "~/k:/k", "${QD_SECRETS_DIR:?}/s:/s", "/anon",
            {"type": "bind", "source": "./y"}, {"type": "volume", "source": "v"},
            {"type": "bind", "source": "/abs"}]"#,
    )
    .unwrap();
    assert_eq!(mounted(&volumes).unwrap(), ["x.sh", "d", "", "y"]);
    let out: Vec<serde_json::Value> = serde_json::from_str(r#"["./a/../../x:/x"]"#).unwrap();
    assert!(mounted(&out).unwrap_err().contains("leaves the package"));
    let up: Vec<serde_json::Value> = serde_json::from_str(r#"["../x:/x"]"#).unwrap();
    assert!(mounted(&up).is_err());
}

#[test]
fn a_directory_gives_every_regular_file_under_it_and_a_link_is_refused() {
    let d = dir("walk");
    let p = Dir(d.clone());
    let names = |found: Vec<(String, Vec<u8>)>| -> Vec<String> {
        found.into_iter().map(|(p, _)| p).collect()
    };
    assert_eq!(
        names(p.files_under("compose/a.sh").unwrap()),
        ["compose/a.sh"]
    );
    assert_eq!(
        names(p.files_under("compose").unwrap()),
        ["compose/a.sh", "compose/inner/b.conf"]
    );
    assert!(p.files_under("nowhere").is_err());
    std::os::unix::fs::symlink("/etc/hostname", d.join("compose/link")).unwrap();
    assert!(p
        .files_under("compose")
        .unwrap_err()
        .contains("not a regular file"));
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn compose_yml_comes_first_and_each_file_once() {
    let d = dir("collect");
    let paths = vec!["compose".to_string(), "compose/a.sh".to_string()];
    let files = collect("services: {}\n", &paths, &Dir(d.clone())).unwrap();
    let got: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(got, ["compose.yml", "compose/a.sh", "compose/inner/b.conf"]);
    assert_eq!(files[2].size, 2);
    std::fs::remove_dir_all(d).unwrap();
}
