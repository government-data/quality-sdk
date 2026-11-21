//! The internal files replaced together, a cut finished or dropped, and
//! when a certificate ends.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rcgen::{date_time_ymd, CertificateParams, KeyPair};

use super::{not_after, staged, Contents, InternalFiles};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-internal-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn files(d: &Path) -> InternalFiles {
    InternalFiles {
        ca: d.join("ca.pem"),
        certificate: d.join("certificate.pem"),
        private_key: d.join("private-key.pem"),
    }
}

fn contents(tag: &str) -> Contents {
    Contents {
        ca: format!("ca {tag}"),
        certificate: format!("certificate {tag}"),
        private_key: format!("key {tag}"),
    }
}

fn read(f: &InternalFiles) -> [String; 3] {
    [&f.ca, &f.certificate, &f.private_key].map(|p| std::fs::read_to_string(p).unwrap())
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// A self-signed certificate that ends on `year`-01-02.
fn certificate(year: i32) -> String {
    let mut params = CertificateParams::new(vec!["agent".to_string()]).unwrap();
    params.not_after = date_time_ymd(year, 1, 2);
    params
        .self_signed(&KeyPair::generate().unwrap())
        .unwrap()
        .pem()
}

#[test]
fn the_three_files_are_replaced_and_the_key_is_private() {
    let d = dir("replace");
    let f = files(&d);
    f.replace(&contents("one")).unwrap();
    f.replace(&contents("two")).unwrap();
    assert_eq!(read(&f), ["ca two", "certificate two", "key two"]);
    assert_eq!(mode(&f.private_key), 0o600);
    assert_eq!(mode(&f.certificate), 0o644);
    let left: Vec<_> = std::fs::read_dir(&d).unwrap().collect();
    assert_eq!(left.len(), 3, "nothing staged is left");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_cut_after_the_key_was_staged_is_finished() {
    let d = dir("forward");
    let f = files(&d);
    f.replace(&contents("old")).unwrap();
    // Every new file staged, and the CA already renamed when it was cut.
    std::fs::write(&f.ca, "ca new").unwrap();
    std::fs::write(staged(&f.certificate), "certificate new").unwrap();
    std::fs::write(staged(&f.private_key), "key new").unwrap();
    f.finish_pending().unwrap();
    assert_eq!(read(&f), ["ca new", "certificate new", "key new"]);
    assert!(!staged(&f.private_key).exists());
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_cut_before_the_key_was_staged_keeps_the_old_files() {
    let d = dir("back");
    let f = files(&d);
    f.replace(&contents("old")).unwrap();
    std::fs::write(staged(&f.ca), "ca new").unwrap();
    std::fs::write(staged(&f.certificate), "certificate new").unwrap();
    f.finish_pending().unwrap();
    assert_eq!(read(&f), ["ca old", "certificate old", "key old"]);
    assert!(!staged(&f.ca).exists() && !staged(&f.certificate).exists());
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn when_a_certificate_ends_is_read_from_the_first_of_its_file() {
    let chain = format!("{}{}", certificate(2031), certificate(2040));
    let end = not_after(chain.as_bytes()).unwrap();
    assert_eq!(end.to_rfc3339(), "2031-01-02T00:00:00+00:00");
    let d = dir("ends");
    let f = files(&d);
    std::fs::write(&f.certificate, &chain).unwrap();
    assert_eq!(f.not_after().unwrap(), end);
    std::fs::write(&f.certificate, "not a certificate").unwrap();
    assert!(f.not_after().unwrap_err().contains("certificate.pem"));
    std::fs::remove_file(&f.certificate).unwrap();
    assert!(f.not_after().is_err());
    std::fs::remove_dir_all(d).unwrap();
}
