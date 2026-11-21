//! Packages made here with the `tar` crate, under a test directory.

use std::os::unix::fs::PermissionsExt;

use super::*;

pub(crate) fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-archive-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A package at `dir/<version>.tar.gz` with `members` (path, text, mode).
pub(crate) fn package(dir: &Path, version: &str, members: &[(&str, &str, u32)]) -> PathBuf {
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

#[test]
fn a_package_is_listed_extracted_with_its_modes_and_packed_back() {
    let d = dir("roundtrip");
    let p = package(
        &d,
        "1.3.0",
        &[
            ("compose.yml", "v1", 0o600),
            ("./bin/run", "#!/bin/sh", 0o750),
        ],
    );
    assert_eq!(listing(&p).unwrap(), ["bin/run", "compose.yml"]);
    extract(&p, &d.join("out")).unwrap();
    let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(d.join("out/bin/run")), 0o755);
    assert_eq!(mode(d.join("out/compose.yml")), 0o644, "whatever the umask");
    assert_eq!(
        std::fs::read_to_string(d.join("out/compose.yml")).unwrap(),
        "v1"
    );

    let copy = d.join("saved/1.3.0.tar.gz");
    assert_eq!(
        pack(&d.join("out"), &copy).unwrap(),
        ["bin/run", "compose.yml"]
    );
    assert_eq!(listing(&copy).unwrap(), ["bin/run", "compose.yml"]);
    assert!(!d.join("saved/1.3.0.tar.gz.partial").exists());
    std::fs::remove_dir_all(d).unwrap();
}

fn raw(d: &Path, name: &str, build: impl FnOnce(&mut Builder<GzEncoder<File>>)) -> PathBuf {
    let path = d.join(name);
    let mut tar = Builder::new(GzEncoder::new(
        File::create(&path).unwrap(),
        flate2::Compression::default(),
    ));
    build(&mut tar);
    tar.into_inner().unwrap().finish().unwrap();
    path
}

#[test]
fn a_link_a_path_out_of_the_package_or_no_file_is_refused() {
    let d = dir("refused");
    let link = raw(&d, "link.tar.gz", |tar| {
        let mut h = Header::new_gnu();
        h.set_entry_type(EntryType::Symlink);
        h.set_size(0);
        tar.append_link(&mut h, "compose.yml", "/etc/passwd")
            .unwrap();
    });
    assert!(listing(&link).unwrap_err().contains("not a file"));
    let up = raw(&d, "up.tar.gz", |tar| {
        let mut h = Header::new_gnu();
        h.set_size(1);
        // `append_data` refuses `..`: the raw name is what a hostile tar has.
        h.as_gnu_mut().unwrap().name[..5].copy_from_slice(b"../x\0");
        h.set_cksum();
        tar.append(&h, "x".as_bytes()).unwrap();
    });
    assert!(listing(&up).unwrap_err().contains("may not have"));
    let empty = package(&d, "empty", &[]);
    assert!(listing(&empty).unwrap_err().contains("no file"));
    std::fs::write(d.join("garbage.tar.gz"), "not a package").unwrap();
    assert!(listing(&d.join("garbage.tar.gz")).is_err());
    std::fs::remove_dir_all(d).unwrap();
}
