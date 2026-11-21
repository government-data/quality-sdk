//! A test CA made here, never a real one.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use p256::ecdsa::{DerSignature, SigningKey};
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use x509_cert::builder::{Builder, CertificateBuilder, Profile};
use x509_cert::der::EncodePem;
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::time::Validity;

use sdk_cli::pki::{public_key_info, Ca};

/// An empty directory for one test.
pub(crate) fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sdk-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) fn name(s: &str) -> Name {
    s.parse().unwrap()
}

/// A self-signed test CA valid for `days`, its key made from `seed`,
/// written as PEM under `dir`.
pub(crate) fn test_ca_with(dir: &Path, days: u64, seed: u8) -> (PathBuf, PathBuf) {
    let key = SigningKey::from_bytes(&[seed; 32].into()).unwrap();
    let cert = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(1u32),
        Validity::from_now(Duration::from_secs(days * 86_400)).unwrap(),
        name("CN=test installations CA,O=quality-data test"),
        public_key_info(key.verifying_key()).unwrap(),
        &key,
    )
    .unwrap()
    .build::<DerSignature>()
    .unwrap();
    let (c, k) = (
        dir.join(format!("ca-{seed}.pem")),
        dir.join(format!("ca-{seed}.key")),
    );
    std::fs::write(&c, cert.to_pem(LineEnding::LF).unwrap()).unwrap();
    std::fs::write(&k, key.to_pkcs8_pem(LineEnding::LF).unwrap().as_bytes()).unwrap();
    (c, k)
}

pub(crate) fn test_ca(dir: &Path, days: u64) -> (PathBuf, PathBuf) {
    test_ca_with(dir, days, 3)
}

pub(crate) fn ca(dir: &Path) -> Ca {
    let (c, k) = test_ca(dir, 3650);
    Ca::load(&c, &k).unwrap()
}
