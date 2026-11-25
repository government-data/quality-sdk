use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::*;
use crate::paths::Paths;

/// A temporary installation root, removed when dropped.
struct Root(PathBuf);

impl Root {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "qs-module-secrets-{name}-{}-{}",
            std::process::id(),
            rand_core::OsRng.next_u32()
        ));
        std::fs::create_dir_all(dir.join("opt/quality-data/bi")).unwrap();
        Self(dir)
    }
    fn secrets(&self) -> ModuleSecrets {
        ModuleSecrets::new(&Paths::under(&self.0))
    }
    fn module(&self) -> PathBuf {
        self.0.join("opt/quality-data/bi")
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        // The files are 0400 and the folder 0700: their owner removes them.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o7777
}

fn rnd(bytes: u32) -> SecretContent {
    SecretContent::Random { bytes }
}

/// The uid these tests run as.
fn me(root: &Root) -> u32 {
    std::fs::metadata(&root.0).unwrap().uid()
}

#[test]
fn a_file_is_made_once_in_a_folder_0700_with_mode_0400() {
    let root = Root::new("once");
    let s = root.secrets();
    let uid = me(&root);
    assert!(s.ensure("bi", "secret-key", rnd(32), uid).unwrap());
    let dir = root.module().join("secrets");
    assert_eq!(mode(&dir), 0o700);
    let file = dir.join("secret-key");
    assert_eq!(mode(&file), 0o400);
    assert_eq!(std::fs::metadata(&file).unwrap().uid(), uid);
    let first = s.read("bi", "secret-key").unwrap();
    // 32 bytes, base64url without padding or newline.
    assert_eq!(first.expose().len(), 43);
    assert!(first
        .expose()
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')));
    // An existing file is never replaced.
    assert!(!s.ensure("bi", "secret-key", rnd(64), uid).unwrap());
    assert_eq!(s.read("bi", "secret-key").unwrap(), first);
    // Nothing is left aside.
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["secret-key"]);
    // A second file has another value.
    s.ensure("bi", "other", rnd(32), uid).unwrap();
    assert_ne!(s.read("bi", "other").unwrap(), first);
}

#[test]
fn a_temporary_file_left_by_a_crash_is_replaced() {
    let root = Root::new("crash");
    let s = root.secrets();
    let dir = s.ensure_dir("bi").unwrap();
    std::fs::write(dir.join(".secret-key.tmp"), "half").unwrap();
    s.ensure("bi", "secret-key", rnd(32), me(&root)).unwrap();
    assert_eq!(s.read("bi", "secret-key").unwrap().expose().len(), 43);
    assert!(!dir.join(".secret-key.tmp").exists());
}

#[test]
fn the_owner_is_the_manifests_uid_when_run_as_root() {
    let root = Root::new("owner");
    let s = root.secrets();
    if me(&root) == 0 {
        s.ensure("bi", "secret-key", rnd(32), 1000).unwrap();
        let file = root.module().join("secrets/secret-key");
        assert_eq!(std::fs::metadata(&file).unwrap().uid(), 1000);
        assert_eq!(
            std::fs::metadata(root.module().join("secrets"))
                .unwrap()
                .uid(),
            0
        );
    } else {
        let other = me(&root) + 1;
        let err = s.ensure("bi", "secret-key", rnd(32), other).unwrap_err();
        assert!(err.contains("only root"), "{err}");
    }
}

#[test]
fn a_next_value_is_reused_after_a_crash_then_renamed_over() {
    let root = Root::new("next");
    let s = root.secrets();
    let uid = me(&root);
    s.ensure("bi", "oidc-people-secret", rnd(32), uid).unwrap();
    let old = s.read("bi", "oidc-people-secret").unwrap();
    assert_eq!(s.read_next("bi", "oidc-people-secret").unwrap(), None);
    s.stage_next("bi", "oidc-people-secret", 32).unwrap();
    let next = s.read_next("bi", "oidc-people-secret").unwrap().unwrap();
    assert_ne!(next, old);
    let file = root.module().join("secrets/oidc-people-secret.next");
    assert_eq!(mode(&file), 0o400);
    // Staged again (a crash before it was used): the same value.
    s.stage_next("bi", "oidc-people-secret", 32).unwrap();
    assert_eq!(
        s.read_next("bi", "oidc-people-secret").unwrap().unwrap(),
        next
    );
    s.promote_next("bi", "oidc-people-secret", uid).unwrap();
    assert_eq!(s.read("bi", "oidc-people-secret").unwrap(), next);
    assert_eq!(s.read_next("bi", "oidc-people-secret").unwrap(), None);
    assert_eq!(
        mode(&root.module().join("secrets/oidc-people-secret")),
        0o400
    );
    // Promoted again (a crash after the rename): nothing changes.
    s.promote_next("bi", "oidc-people-secret", uid).unwrap();
    assert_eq!(s.read("bi", "oidc-people-secret").unwrap(), next);
}

#[test]
fn the_env_gets_the_agents_keys_and_keeps_every_other_line() {
    let root = Root::new("env");
    let s = root.secrets();
    let env = root.module().join(".env");
    std::fs::write(
        &env,
        "# the module's\nIMAGE_WEB=r/web@sha256:00\nQD_DB_NAME=old\nMY_SETTING=1\n\n",
    )
    .unwrap();
    let vars = [
        ("QD_SECRETS_DIR", "/opt/quality-data/bi/secrets".to_string()),
        ("QD_DB_NAME", "bi".to_string()),
    ];
    s.write_env("bi", &vars).unwrap();
    assert_eq!(
        std::fs::read_to_string(&env).unwrap(),
        "# the module's\nIMAGE_WEB=r/web@sha256:00\nMY_SETTING=1\n\
         QD_SECRETS_DIR=/opt/quality-data/bi/secrets\nQD_DB_NAME=bi\n"
    );
    assert_eq!(mode(&env), 0o600);
    s.write_env("bi", &vars).unwrap();
    assert_eq!(
        std::fs::read_to_string(&env)
            .unwrap()
            .matches("QD_")
            .count(),
        2
    );
    let err = s
        .write_env("bi", &[("DB_PASSWORD", "x".to_string())])
        .unwrap_err();
    assert!(err.contains("cannot go in its .env"), "{err}");
    assert!(s
        .write_env("bi", &[("QD_DB_NAME", "a\nB=1".to_string())])
        .is_err());
}

#[test]
fn the_ca_is_copied_public() {
    let root = Root::new("ca");
    let paths = Paths::under(&root.0);
    std::fs::create_dir_all(paths.internal_ca().parent().unwrap()).unwrap();
    std::fs::write(paths.internal_ca(), "-----BEGIN CERTIFICATE-----\n").unwrap();
    let s = root.secrets();
    let ca = s.copy_ca("bi").unwrap();
    assert_eq!(ca, root.module().join("ca.crt"));
    assert_eq!(mode(&ca), 0o444);
    // Copied again (a new CA): replaced.
    std::fs::write(paths.internal_ca(), "-----BEGIN CERTIFICATE-----\nnew\n").unwrap();
    s.copy_ca("bi").unwrap();
    assert!(std::fs::read_to_string(&ca).unwrap().ends_with("new\n"));
}

#[test]
fn no_value_is_printed_or_named_in_an_error() {
    let root = Root::new("quiet");
    let s = root.secrets();
    s.ensure("bi", "secret-key", rnd(32), me(&root)).unwrap();
    let value = s.read("bi", "secret-key").unwrap();
    assert_eq!(format!("{value:?}"), "[secret]");
    // Every error names the file and the step, never the content.
    std::fs::set_permissions(
        root.module().join("secrets"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    if me(&root) != 0 {
        let err = s.read("bi", "secret-key").unwrap_err();
        assert!(err.contains("secret-key (read)"), "{err}");
        assert!(!err.contains(value.expose()));
    }
    std::fs::set_permissions(
        root.module().join("secrets"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
}

#[test]
fn a_key_is_pkcs8_base64_that_parses_as_its_kind_and_is_kept() {
    use base64::engine::general_purpose::STANDARD;
    use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
    let root = Root::new("keys");
    let s = root.secrets();
    let uid = me(&root);
    assert!(s.ensure("bi", "ed", SecretContent::Ed25519, uid).unwrap());
    assert!(s.ensure("bi", "es", SecretContent::P256, uid).unwrap());
    let dir = root.module().join("secrets");
    for name in ["ed", "es"] {
        assert_eq!(mode(&dir.join(name)), 0o400, "{name}");
    }
    let der = |name: &str| {
        let text = s.read("bi", name).unwrap().expose().to_string();
        assert!(!text.contains(['\n', '\r']), "{name}");
        STANDARD.decode(text).unwrap()
    };
    let ed = der("ed");
    Ed25519KeyPair::from_pkcs8_maybe_unchecked(&ed).unwrap();
    assert!(EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        &ed,
        &ring::rand::SystemRandom::new()
    )
    .is_err());
    let es = der("es");
    EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        &es,
        &ring::rand::SystemRandom::new(),
    )
    .unwrap();
    assert!(Ed25519KeyPair::from_pkcs8_maybe_unchecked(&es).is_err());
    // A file that exists is kept, whatever kind is asked.
    assert!(!s.ensure("bi", "ed", SecretContent::Ed25519, uid).unwrap());
    assert!(!s.ensure("bi", "es", rnd(32), uid).unwrap());
    assert_eq!(der("ed"), ed);
    assert_eq!(der("es"), es);
}
