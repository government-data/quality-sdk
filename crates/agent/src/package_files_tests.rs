//! The package files under a test root: packages made here with the `tar`
//! crate, nothing spawned. What a package may hold is tested with
//! `package_archive`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use flate2::write::GzEncoder;
use tar::{Builder, EntryType, Header};

use crate::package_archive::files_under;

use super::*;
use crate::paths::Paths;

fn root(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn files(root: &Path, free: u64) -> PackageFiles {
    let mut f = PackageFiles::new(&Paths::under(root));
    f.free_bytes = Box::new(move |_| Ok(free));
    f
}

/// A package at `dir/<version>.tar.gz` with `members` (path, text, mode).
fn package(dir: &Path, version: &str, members: &[(&str, &str, u32)]) -> PathBuf {
    let path = dir.join(format!("{version}.tar.gz"));
    let mut tar = Builder::new(GzEncoder::new(
        File::create(&path).unwrap(),
        flate2::Compression::default(),
    ));
    for (name, text, mode) in members {
        let mut h = Header::new_gnu();
        h.set_size(text.len() as u64);
        h.set_mode(*mode);
        h.set_cksum();
        tar.append_data(&mut h, name, text.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    path
}

fn read(path: PathBuf) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn a_package_is_put_whole_and_current_points_at_it() {
    let r = root("put");
    let f = files(&r, MINIMUM_FREE);
    let qd = r.join("opt/quality-data/quality-data");
    std::fs::create_dir_all(qd.join("secrets")).unwrap();
    std::fs::write(qd.join(".env"), "IMAGE_API=x\n").unwrap();
    let p = package(
        &r,
        "1.3.0",
        &[
            ("compose.yml", "v1", 0o644),
            ("./bin/run", "#!/bin/sh", 0o750),
        ],
    );
    assert_eq!(f.manifest(&p).unwrap(), ["bin/run", "compose.yml"]);
    f.put("quality-data", &p, &[]).unwrap();
    assert_eq!(
        std::fs::read_link(qd.join("current")).unwrap(),
        Path::new("1.3.0")
    );
    assert_eq!(read(qd.join("current/compose.yml")), "v1");
    let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(qd.join("1.3.0/bin/run")), 0o755);
    assert_eq!(mode(qd.join("1.3.0/compose.yml")), 0o644);
    assert_eq!(read(qd.join(".env")), "IMAGE_API=x\n", "the module's own");
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn the_previous_version_is_kept_older_ones_go_and_the_saved_copy_goes_back() {
    let r = root("versions");
    let f = files(&r, MINIMUM_FREE);
    let qd = r.join("opt/quality-data/quality-data");
    for v in ["1.2.0", "1.3.0"] {
        let p = package(
            &r,
            v,
            &[("compose.yml", v, 0o644), (&format!("only-{v}"), v, 0o644)],
        );
        f.put("quality-data", &p, &[]).unwrap();
    }
    std::fs::create_dir_all(qd.join("secrets")).unwrap();
    let (saved, manifest) = f.save("quality-data", "1.3.0").unwrap();
    assert_eq!(
        saved,
        r.join("var/lib/quality-data/updater/releases/quality-data/1.3.0.tar.gz")
    );
    assert_eq!(manifest, ["compose.yml", "only-1.3.0"]);
    assert_eq!(f.manifest(&saved).unwrap(), manifest);
    assert!(f
        .save("quality-data", "1.2.0")
        .unwrap_err()
        .contains("not 1.2.0's"));

    let p = package(&r, "1.4.0", &[("compose.yml", "1.4.0", 0o644)]);
    f.put("quality-data", &p, &["only-1.3.0".into()]).unwrap();
    assert!(qd.join("1.3.0").is_dir(), "the previous one is kept");
    assert!(!qd.join("1.2.0").exists(), "older ones go");
    assert!(
        qd.join("secrets").is_dir(),
        "a name that is no version stays"
    );
    assert!(!qd.join("current/only-1.3.0").exists());

    // Back to the saved copy: the kept directory serves.
    f.put("quality-data", &saved, &[]).unwrap();
    assert_eq!(read(qd.join("current/compose.yml")), "1.3.0");
    assert!(qd.join("current/only-1.3.0").exists());
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn a_package_that_is_refused_changes_nothing() {
    let r = root("refused");
    let f = files(&r, MINIMUM_FREE);
    let ok = package(&r, "1.0.0", &[("compose.yml", "ok", 0o644)]);
    f.put("quality-data", &ok, &[]).unwrap();

    let link = r.join("2.0.0.tar.gz");
    let mut tar = Builder::new(GzEncoder::new(
        File::create(&link).unwrap(),
        flate2::Compression::default(),
    ));
    let mut h = Header::new_gnu();
    h.set_entry_type(EntryType::Symlink);
    h.set_size(0);
    tar.append_link(&mut h, "compose.yml", "/etc/passwd")
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    assert!(f
        .put("quality-data", &link, &[])
        .unwrap_err()
        .contains("not a file"));
    let misnamed = r.join("package.tgz");
    std::fs::copy(&ok, &misnamed).unwrap();
    assert!(f.put("quality-data", &misnamed, &[]).is_err());
    assert!(f.put("../x", &ok, &[]).is_err());
    assert!(f.put("current", &ok, &[]).is_err());

    let qd = r.join("opt/quality-data/quality-data");
    assert_eq!(
        std::fs::read_link(qd.join("current")).unwrap(),
        Path::new("1.0.0")
    );
    assert!(!qd.join("2.0.0").exists());
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn what_a_crash_left_halfway_is_replaced() {
    let r = root("crash");
    let f = files(&r, MINIMUM_FREE);
    let qd = r.join("opt/quality-data/quality-data");
    std::fs::create_dir_all(qd.join(".1.3.0.partial")).unwrap();
    std::fs::write(qd.join(".1.3.0.partial/stale"), "x").unwrap();
    std::fs::create_dir_all(qd.join("1.3.0")).unwrap();
    std::fs::write(qd.join("1.3.0/half"), "x").unwrap();
    let p = package(&r, "1.3.0", &[("compose.yml", "whole", 0o644)]);
    f.put("quality-data", &p, &[]).unwrap();
    f.put("quality-data", &p, &[]).unwrap();
    assert_eq!(files_under(&qd.join("current")).unwrap(), ["compose.yml"]);
    assert!(!qd.join(".1.3.0.partial").exists());
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn an_update_needs_two_gigabytes_on_each_disk() {
    let r = root("space");
    files(&r, MINIMUM_FREE).free_space().unwrap();
    let err = files(&r, 100 << 20).free_space().unwrap_err();
    assert!(
        err.contains("100 MB free") && err.contains("2048 MB"),
        "{err}"
    );
    assert_eq!(
        parse_df(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda 100 40 60 40% /\n"
        ),
        Ok(60 * 1024)
    );
    assert!(parse_df("").is_err());
    std::fs::remove_dir_all(r).unwrap();
}
