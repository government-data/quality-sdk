//! The installations' CA and what it signs (decision 0001): keys are ECDSA
//! P-256, a certificate's subject is OU `installation` and CN the
//! installation's id, and the CA's key is read only where it is kept,
//! offline.

use std::path::Path;

use p256::ecdsa::{SigningKey, VerifyingKey};
use rand_core::{OsRng, RngCore};
use x509_cert::certificate::Certificate;
use x509_cert::der::oid::db::rfc4519;
use x509_cert::der::{DecodePem, Tag, Tagged};
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::SubjectPublicKeyInfoOwned;

// The installation's subject and the key and file helpers are the agent's
// (`qd enrol`, decision 0008): one definition for what is enrolled and
// what is signed.
pub use agent::enrol::{
    installation_subject, signing_key_from_pem, write_private, write_replacing, INSTALLATION_OU,
};

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
