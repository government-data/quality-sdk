//! The installations' CA and what it signs (decision 0001): keys are ECDSA
//! P-256, a certificate's subject is OU `installation` and CN the
//! installation's id, and the CA's key is read only where it is kept,
//! offline.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use p256::ecdsa::{SigningKey, VerifyingKey};
use p256::pkcs8::DecodePrivateKey;
use p256::SecretKey;
use rand_core::OsRng;
use x509_cert::attr::AttributeTypeAndValue;
use x509_cert::der::asn1::{Any, SetOfVec};
use x509_cert::der::oid::db::rfc4519;
use x509_cert::der::Tag;
use x509_cert::name::{Name, RdnSequence, RelativeDistinguishedName};
use x509_cert::spki::SubjectPublicKeyInfoOwned;

/// The OU every installation's certificate carries; Roles Anywhere's trust
/// policy accepts no other.
pub const INSTALLATION_OU: &str = "installation";

/// A P-256 private key from PEM: SEC1 (`EC PRIVATE KEY`) or PKCS #8.
pub fn signing_key_from_pem(pem: &str) -> Result<SigningKey, String> {
    let secret = if pem.contains("BEGIN EC PRIVATE KEY") {
        SecretKey::from_sec1_pem(pem).map_err(|e| e.to_string())
    } else {
        SecretKey::from_pkcs8_pem(pem).map_err(|e| e.to_string())
    };
    secret
        .map(SigningKey::from)
        .map_err(|e| format!("not a P-256 private key: {e}"))
}

pub fn public_key_info(key: &VerifyingKey) -> Result<SubjectPublicKeyInfoOwned, String> {
    SubjectPublicKeyInfoOwned::from_key(*key).map_err(|e| e.to_string())
}

/// A new key from the operating system's randomness.
pub fn new_key() -> SigningKey {
    SigningKey::random(&mut OsRng)
}

/// An installation's subject: OU `installation`, CN its id.
pub fn installation_subject(installation: &str) -> Result<Name, String> {
    let rdn = |oid, value: &str| -> Result<RelativeDistinguishedName, String> {
        let value = Any::new(Tag::Utf8String, value.as_bytes()).map_err(|e| e.to_string())?;
        let set = SetOfVec::try_from(vec![AttributeTypeAndValue { oid, value }])
            .map_err(|e| e.to_string())?;
        Ok(RelativeDistinguishedName(set))
    };
    Ok(RdnSequence(vec![
        rdn(rfc4519::OU, INSTALLATION_OU)?,
        rdn(rfc4519::CN, installation)?,
    ]))
}

pub fn read(file: &Path) -> Result<String, String> {
    std::fs::read_to_string(file).map_err(|e| format!("could not read {}: {e}", path(file)))
}

pub fn path(file: &Path) -> String {
    file.display().to_string()
}

/// Write `content` to a new file only its owner may read (0600), in a
/// directory only its owner may enter when it is created (0700). An
/// existing file is never overwritten.
pub fn write_private(file: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("could not create {}: {e}", path(dir)))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file)
        .map_err(|e| format!("could not create {}: {e}", path(file)))?;
    f.write_all(content)
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("could not write {}: {e}", path(file)))
}

/// Write `content` whole: to a temporary file renamed over `file`.
pub fn write_replacing(file: &Path, content: &[u8]) -> Result<(), String> {
    let temporary = file.with_extension("tmp");
    let write = || -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::File::create(&temporary)?;
        f.write_all(content)?;
        f.sync_all()?;
        std::fs::rename(&temporary, file)
    };
    write().map_err(|e| format!("could not write {}: {e}", path(file)))
}
