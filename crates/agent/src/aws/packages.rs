//! A module's package (decision 0003: one per module): downloaded from the
//! release bucket and kept only when its SHA-256 is the one the signed index
//! names. Downloading again a package already kept and whole reads nothing.

use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use updater::engine::OfferedPackage;

use super::client::Bucket;

/// Where a module's package at `version` is kept under `dir`
/// (`/var/lib/quality-data/updater/packages`).
pub fn package_path(dir: &Path, module: &str, version: &str) -> Result<PathBuf, String> {
    let safe = |s: &str| {
        !s.is_empty()
            && !s.starts_with('.')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
    };
    if !safe(module) || !safe(version) {
        return Err(format!("{module:?} at {version:?} cannot name a file"));
    }
    Ok(dir.join(module).join(format!("{version}.tar.gz")))
}

/// Download `package` of a platform `version` into `dir`, check its SHA-256
/// and leave it whole (a temporary file renamed); a package that does not
/// match is not kept.
pub fn download(
    bucket: &Bucket,
    package: &OfferedPackage,
    version: &str,
    dir: &Path,
) -> Result<PathBuf, String> {
    let path = package_path(dir, &package.module, version)?;
    if std::fs::read(&path).is_ok_and(|kept| sha256(&kept) == package.sha256) {
        return Ok(path);
    }
    let bytes = bucket
        .get(&package.key)
        .map_err(|e| format!("{}: {e}", package.key))?;
    let got = sha256(&bytes);
    if got != package.sha256 {
        return Err(format!(
            "{}: the SHA-256 is {got}, the index says {}",
            package.key, package.sha256
        ));
    }
    let temporary = path.with_extension("gz.tmp");
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&temporary)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        std::fs::rename(&temporary, &path)
    };
    write().map_err(|e| format!("could not keep {}: {e}", path.display()))?;
    Ok(path)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_client::fake::{bucket, FakeAws};

    fn package(body: &[u8]) -> OfferedPackage {
        OfferedPackage {
            module: "orchestrator".into(),
            key: "packages/orchestrator/1.4.0.tar.gz".into(),
            sha256: sha256(body),
            provides: Vec::new(),
            requires: Vec::new(),
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agent-packages-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_package_is_kept_when_its_sha256_matches_and_not_read_twice() {
        let d = dir("ok");
        let aws = FakeAws::default();
        aws.answer("GET /packages/orchestrator/1.4.0.tar.gz", 200, b"package");
        let p = package(b"package");
        let path = download(&bucket(&aws), &p, "1.4.0", &d).unwrap();
        assert_eq!(path, d.join("orchestrator/1.4.0.tar.gz"));
        assert_eq!(std::fs::read(&path).unwrap(), b"package");
        // Again: already whole, nothing is asked.
        download(&bucket(&aws), &p, "1.4.0", &d).unwrap();
        assert_eq!(aws.sent.borrow().len(), 1);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_package_that_does_not_match_is_not_kept() {
        let d = dir("bad");
        let aws = FakeAws::default();
        aws.answer("GET /packages/orchestrator/1.4.0.tar.gz", 200, b"changed");
        let err = download(&bucket(&aws), &package(b"package"), "1.4.0", &d).unwrap_err();
        assert!(err.contains("the SHA-256 is"), "{err}");
        assert!(!d.join("orchestrator/1.4.0.tar.gz").exists());
        // A damaged copy left on disk is downloaded again.
        std::fs::create_dir_all(d.join("orchestrator")).unwrap();
        std::fs::write(d.join("orchestrator/1.4.0.tar.gz"), b"half").unwrap();
        aws.answers.borrow_mut()[0].1.body = b"package".to_vec();
        download(&bucket(&aws), &package(b"package"), "1.4.0", &d).unwrap();
        assert_eq!(
            std::fs::read(d.join("orchestrator/1.4.0.tar.gz")).unwrap(),
            b"package"
        );
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_denied_package_or_a_name_that_is_not_a_file_fails() {
        let d = dir("denied");
        let aws = FakeAws::default();
        aws.answer("GET /packages/orchestrator/1.4.0.tar.gz", 403, b"");
        let err = download(&bucket(&aws), &package(b"x"), "1.4.0", &d).unwrap_err();
        assert!(err.contains("denied"), "{err}");
        assert!(package_path(&d, "../etc", "1.0.0").is_err());
        assert!(package_path(&d, "m", "../1").is_err());
    }
}
