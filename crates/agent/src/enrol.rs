//! `qd enrol`, on the installation's server (decisions 0001 and 0008): a
//! P-256 key made here and kept here (`enrolment/private-key.pem`, PKCS #8,
//! 0600), and a certificate request for the CA's holder to sign with `qsdk
//! sign-enrolment`, with OU `installation` and CN the installation's id.
//! Run again, it keeps the key and writes the request again.
//!
//! The installation's subject and the key and file helpers live here once:
//! `qsdk` uses them to check and sign what this writes.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use p256::ecdsa::{DerSignature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use p256::SecretKey;
use x509_cert::attr::AttributeTypeAndValue;
use x509_cert::builder::{Builder, RequestBuilder};
use x509_cert::der::asn1::{Any, SetOfVec};
use x509_cert::der::oid::db::rfc4519;
use x509_cert::der::{EncodePem, Tag};
use x509_cert::name::{Name, RdnSequence, RelativeDistinguishedName};

use crate::paths::Paths;

/// The OU every installation's certificate carries; Roles Anywhere's trust
/// policy accepts no other.
pub const INSTALLATION_OU: &str = "installation";

/// What enrolling left: the request, as PEM, and whether the key is new.
#[derive(Debug)]
pub struct Enrolled {
    pub request_pem: String,
    pub new_key: bool,
}

/// The key, made once, and the request, written each time.
pub fn run(paths: &Paths, installation: &str) -> Result<Enrolled, String> {
    crate::config::check_installation(installation)?;
    let key_file = paths.private_key();
    let (key, new_key) = if key_file.exists() {
        let pem = std::fs::read_to_string(&key_file)
            .map_err(|e| format!("could not read {}: {e}", key_file.display()))?;
        (signing_key_from_pem(&pem)?, false)
    } else {
        let key = SigningKey::random(&mut rand_core::OsRng);
        // PKCS #8, which names the curve: OpenSSL and the signing helper
        // read it.
        let pem = key
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| e.to_string())?;
        write_private(&key_file, pem.as_bytes())?;
        (key, true)
    };
    let request = RequestBuilder::new(installation_subject(installation)?, &key)
        .and_then(|b| b.build::<DerSignature>())
        .map_err(|e| format!("could not build the certificate request: {e}"))?;
    let request_pem = request.to_pem(LineEnding::LF).map_err(|e| e.to_string())?;
    write_replacing(&paths.certificate_request(), request_pem.as_bytes())?;
    Ok(Enrolled {
        request_pem,
        new_key,
    })
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

/// Write `content` to a new file only its owner may read (0600), in a
/// directory only its owner may enter when it is created (0700). An
/// existing file is never overwritten.
pub fn write_private(file: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file)
        .map_err(|e| format!("could not create {}: {e}", file.display()))?;
    f.write_all(content)
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("could not write {}: {e}", file.display()))
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
    write().map_err(|e| format!("could not write {}: {e}", file.display()))
}
