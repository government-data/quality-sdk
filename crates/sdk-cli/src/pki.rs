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
use rand_core::{OsRng, RngCore};
use x509_cert::attr::AttributeTypeAndValue;
use x509_cert::certificate::Certificate;
use x509_cert::der::asn1::{Any, SetOfVec};
use x509_cert::der::oid::db::rfc4519;
use x509_cert::der::{DecodePem, Tag, Tagged};
use x509_cert::name::{Name, RdnSequence, RelativeDistinguishedName};
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::SubjectPublicKeyInfoOwned;

/// The OU every installation's certificate carries; Roles Anywhere's trust
/// policy accepts no other.
pub const INSTALLATION_OU: &str = "installation";

/// The installations' CA: its certificate and its private key.
pub struct Ca {
    pub certificate: Certificate,
    pub key: SigningKey,
}

/// The key is never printed.
impl std::fmt::Debug for Ca {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ca")
            .field(
                "subject",
                &self.certificate.tbs_certificate.subject.to_string(),
            )
            .finish_non_exhaustive()
    }
}

impl Ca {
    /// Read the CA's certificate and key (PEM, SEC1 or PKCS #8), and check
    /// that the key is the certificate's.
    pub fn load(certificate: &Path, key: &Path) -> Result<Self, String> {
        let pem = read(certificate)?;
        let certificate = Certificate::from_pem(pem.as_bytes())
            .map_err(|e| format!("{} is not a PEM certificate: {e}", path(certificate)))?;
        let key = signing_key_from_pem(&read(key)?).map_err(|e| format!("{}: {e}", path(key)))?;
        let ca = Self { certificate, key };
        if ca.public_key()? != ca.certificate.tbs_certificate.subject_public_key_info {
            return Err("the CA key is not the CA certificate's key".into());
        }
        Ok(ca)
    }

    /// The CA's subject, exactly as its certificate encodes it: what every
    /// certificate and CRL it signs names as issuer.
    pub fn subject(&self) -> &Name {
        &self.certificate.tbs_certificate.subject
    }

    pub fn public_key(&self) -> Result<SubjectPublicKeyInfoOwned, String> {
        public_key_info(self.key.verifying_key())
    }
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

pub fn public_key_info(key: &VerifyingKey) -> Result<SubjectPublicKeyInfoOwned, String> {
    SubjectPublicKeyInfoOwned::from_key(*key).map_err(|e| e.to_string())
}

/// A new key from the operating system's randomness.
pub fn new_key() -> SigningKey {
    SigningKey::random(&mut OsRng)
}

/// A random positive serial of 16 bytes.
pub fn new_serial() -> Result<SerialNumber, String> {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    // Never zero at the front, so the serial keeps its 16 bytes.
    bytes[0] = (bytes[0] & 0x7f) | 0x40;
    SerialNumber::new(&bytes).map_err(|e| e.to_string())
}

/// A serial as people and `openssl x509 -serial` write it: hex, upper case.
pub fn serial_hex(serial: &SerialNumber) -> String {
    serial
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
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

/// The OU and CN values of a subject, in order. Values that are not text
/// are left out.
pub fn subject_values(name: &Name) -> (Vec<String>, Vec<String>) {
    let (mut ou, mut cn) = (Vec::new(), Vec::new());
    for atv in name.0.iter().flat_map(|rdn| rdn.0.iter()) {
        let text = matches!(
            atv.value.tag(),
            Tag::Utf8String | Tag::PrintableString | Tag::Ia5String
        );
        let Some(value) = text
            .then(|| std::str::from_utf8(atv.value.value()).ok())
            .flatten()
        else {
            continue;
        };
        if atv.oid == rfc4519::OU {
            ou.push(value.to_string());
        } else if atv.oid == rfc4519::CN {
            cn.push(value.to_string());
        }
    }
    (ou, cn)
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
