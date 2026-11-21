//! `quality-sdk revoke` (decision 0001): an installation's certificate
//! added to the installations' CRL, signed with the CA, kept in a file and
//! imported into Roles Anywhere, so the installation gets no more
//! credentials.
//!
//! The file holds the list: each revocation reads the previous CRL (it must
//! be this CA's), adds the serial and signs the next CRL number. Roles
//! Anywhere keeps one CRL by name for the trust anchor: imported the first
//! time, updated after. The CRL stays current as long as the CA: its next
//! update is the CA certificate's end.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::SystemTime;

use aws_client::client::AwsError;
use aws_client::rolesanywhere::{Crl, RolesAnywhere};
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{DerSignature, Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use x509_cert::crl::{CertificateList, RevokedCert, TbsCertList};
use x509_cert::der::asn1::{BitString, Uint, UtcTime};
use x509_cert::der::pem::{self, LineEnding};
use x509_cert::der::referenced::OwnedToRef;
use x509_cert::der::{Decode, Encode};
use x509_cert::ext::pkix::{AuthorityKeyIdentifier, CrlNumber};
use x509_cert::ext::{AsExtension, Extension};
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::DynSignatureAlgorithmIdentifier;
use x509_cert::time::Time;
use x509_cert::Version;

use crate::pki::{serial_hex, Ca};

/// Where the CRL file is kept when `--crl` is not given: beside the CA key,
/// backed up with it (decision 0001), under the home directory `home`.
pub fn default_crl(home: Option<&OsStr>) -> Result<PathBuf, String> {
    match home {
        Some(home) if !home.is_empty() => Ok(PathBuf::from(home).join("qd-ca/installations.crl")),
        _ => Err("--crl is required: HOME is not set for its default".into()),
    }
}

/// The next CRL, and what it lists.
#[derive(Debug)]
pub struct NextCrl {
    pub pem: String,
    pub number: u64,
    /// Every revoked serial, hex.
    pub serials: Vec<String>,
    /// Whether the serial was already listed.
    pub already: bool,
}

/// The CRL after `previous` (PEM, or none for the first) with `serial`
/// revoked at `now`.
pub fn next_crl(
    ca: &Ca,
    previous: Option<&str>,
    serial: &SerialNumber,
    now: SystemTime,
) -> Result<NextCrl, String> {
    let (mut revoked, number) = match previous {
        Some(pem) => read_crl(ca, pem)?,
        None => (Vec::new(), 0),
    };
    let already = revoked.iter().any(|r| r.serial_number == *serial);
    let now = Time::UtcTime(UtcTime::from_system_time(now).map_err(|e| e.to_string())?);
    if !already {
        revoked.push(RevokedCert {
            serial_number: serial.clone(),
            revocation_date: now,
            crl_entry_extensions: None,
        });
    }
    let number = number + 1;
    let fail = |e: x509_cert::der::Error| format!("could not build the CRL: {e}");
    let subject = ca.subject();
    let extensions = vec![
        AuthorityKeyIdentifier::try_from(ca.public_key()?.owned_to_ref())
            .and_then(|aki| aki.to_extension(subject, &[]))
            .map_err(fail)?,
        Uint::new(&number.to_be_bytes())
            .and_then(|n| CrlNumber(n).to_extension(subject, &[]))
            .map_err(fail)?,
    ];
    let algorithm = ca
        .key
        .signature_algorithm_identifier()
        .map_err(|e| e.to_string())?;
    let tbs = TbsCertList {
        version: Version::V2,
        signature: algorithm.clone(),
        issuer: subject.clone(),
        this_update: now,
        next_update: Some(ca.certificate.tbs_certificate.validity.not_after),
        revoked_certificates: Some(revoked),
        crl_extensions: Some(extensions),
    };
    let signature: DerSignature = ca.key.sign(&tbs.to_der().map_err(fail)?);
    let serials = tbs
        .revoked_certificates
        .iter()
        .flatten()
        .map(|r| serial_hex(&r.serial_number))
        .collect();
    let crl = CertificateList {
        tbs_cert_list: tbs,
        signature_algorithm: algorithm,
        signature: BitString::from_bytes(signature.as_bytes()).map_err(fail)?,
    };
    let der = crl.to_der().map_err(fail)?;
    let pem = pem::encode_string("X509 CRL", LineEnding::LF, &der).map_err(|e| e.to_string())?;
    Ok(NextCrl {
        pem,
        number,
        serials,
        already,
    })
}

/// A previous CRL's entries and number, once it is known to be this CA's.
fn read_crl(ca: &Ca, pem_text: &str) -> Result<(Vec<RevokedCert>, u64), String> {
    let (label, der) = pem::decode_vec(pem_text.as_bytes())
        .map_err(|e| format!("the CRL file is not PEM: {e}"))?;
    if label != "X509 CRL" {
        return Err(format!("the CRL file holds a {label}, not an X509 CRL"));
    }
    let crl = CertificateList::from_der(&der).map_err(|e| format!("the CRL file: {e}"))?;
    let tbs = &crl.tbs_cert_list;
    let key =
        VerifyingKey::from_public_key_der(&ca.public_key()?.to_der().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let signed = crl
        .signature
        .as_bytes()
        .and_then(|der| Signature::from_der(der).ok())
        .is_some_and(|sig| {
            tbs.to_der()
                .is_ok_and(|bytes| key.verify(&bytes, &sig).is_ok())
        });
    if tbs.issuer != *ca.subject() || !signed {
        return Err("the CRL file is not signed by this CA".into());
    }
    let number = tbs
        .crl_extensions
        .iter()
        .flatten()
        .find(|e| e.extn_id == x509_cert::der::oid::db::rfc5280::ID_CE_CRL_NUMBER)
        .map(crl_number)
        .transpose()?
        .unwrap_or(0);
    Ok((tbs.revoked_certificates.clone().unwrap_or_default(), number))
}

fn crl_number(extension: &Extension) -> Result<u64, String> {
    let n = CrlNumber::from_der(extension.extn_value.as_bytes())
        .map_err(|e| format!("the CRL number: {e}"))?;
    let bytes = n.0.as_bytes();
    if bytes.len() > 8 {
        return Err("the CRL number is too large".into());
    }
    Ok(bytes.iter().fold(0, |n, b| (n << 8) | u64::from(*b)))
}

/// A serial from hex (`AB:CD…` or `abcd…`), as `sign-enrolment` printed it.
pub fn parse_serial(hex_text: &str) -> Result<SerialNumber, String> {
    let digits: String = hex_text.chars().filter(|c| *c != ':').collect();
    let bytes =
        hex::decode(digits.trim()).map_err(|_| format!("serial {hex_text:?} is not hex"))?;
    if bytes.iter().all(|b| *b == 0) {
        return Err(format!("serial {hex_text:?} is zero"));
    }
    SerialNumber::new(&bytes).map_err(|e| format!("serial {hex_text:?}: {e}"))
}

/// What Roles Anywhere was asked to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Uploaded {
    Imported(String),
    Updated(String),
}

/// Put `pem` as the CRL `name` of the trust anchor: updated when it is
/// there (and enabled if it was not), imported otherwise.
pub fn upload(
    ra: &RolesAnywhere,
    name: &str,
    trust_anchor_arn: &str,
    pem: &str,
) -> Result<Uploaded, AwsError> {
    let existing = ra
        .list_crls()?
        .into_iter()
        .find(|c: &Crl| c.name == name && c.trust_anchor_arn == trust_anchor_arn);
    let crl = match existing {
        Some(c) => {
            ra.update_crl(&c.crl_id, pem)?;
            if !c.enabled {
                ra.enable_crl(&c.crl_id)?;
            }
            return Ok(Uploaded::Updated(c.crl_id));
        }
        None => ra.import_crl(name, trust_anchor_arn, pem)?,
    };
    if !crl.enabled {
        ra.enable_crl(&crl.crl_id)?;
    }
    Ok(Uploaded::Imported(crl.crl_id))
}
