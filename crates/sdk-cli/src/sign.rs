//! `qsdk sign-enrolment`, by the CA's holder, offline (decision
//! 0001): an installation's certificate request signed with the
//! installations' CA. The request must prove its key (its signature
//! verifies) and name one OU, `installation`, and one CN, a valid
//! installation id; the certificate names exactly those and nothing else
//! the request asked for.

use std::time::Duration;

use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{DerSignature, Signature, VerifyingKey};
use p256::pkcs8::{DecodePublicKey, LineEnding};
use x509_cert::builder::{Builder, CertificateBuilder, Profile};
use x509_cert::der::oid::db::{rfc5280, rfc5912};
use x509_cert::der::oid::{AssociatedOid, ObjectIdentifier};
use x509_cert::der::{DecodePem, Encode, EncodePem, Length, Writer};
use x509_cert::ext::pkix::ExtendedKeyUsage;
use x509_cert::ext::{AsExtension, Extension};
use x509_cert::name::Name;
use x509_cert::request::CertReq;
use x509_cert::time::Validity;

use crate::pki::{
    installation_subject, new_serial, serial_hex, subject_values, Ca, INSTALLATION_OU,
};

/// A signed enrolment.
#[derive(Debug)]
pub struct Signed {
    pub installation: String,
    /// Hex, as `revoke --serial` takes it.
    pub serial: String,
    pub certificate_pem: String,
}

/// What the request asks for, once checked: the installation and its key.
fn check_request(pem: &str) -> Result<(String, VerifyingKey), String> {
    let request = CertReq::from_pem(pem.as_bytes())
        .map_err(|e| format!("not a PEM certificate request: {e}"))?;
    if request.algorithm.oid != rfc5912::ECDSA_WITH_SHA_256 {
        return Err("the request is not signed with ECDSA and SHA-256".into());
    }
    let spki = request
        .info
        .public_key
        .to_der()
        .map_err(|e| e.to_string())?;
    let key = VerifyingKey::from_public_key_der(&spki)
        .map_err(|_| "the request's key is not a P-256 key".to_string())?;
    let signature = request
        .signature
        .as_bytes()
        .and_then(|der| Signature::from_der(der).ok())
        .ok_or("the request's signature is malformed")?;
    let info = request.info.to_der().map_err(|e| e.to_string())?;
    key.verify(&info, &signature)
        .map_err(|_| "the request's signature does not verify: it does not hold its key")?;
    let (ou, cn) = subject_values(&request.info.subject);
    if ou != [INSTALLATION_OU] {
        return Err(format!(
            "the request's subject must have OU={INSTALLATION_OU} once (it has {})",
            request.info.subject
        ));
    }
    let [installation] = cn.as_slice() else {
        return Err(format!(
            "the request's subject must have one CN, the installation's id (it has {})",
            request.info.subject
        ));
    };
    agent::config::check_installation(installation)?;
    Ok((installation.clone(), key))
}

/// How long an installation's certificate lasts when `--days` is not given
/// (decision 0001).
pub const DEFAULT_DAYS: u32 = 365;

/// The days to sign for: `given`, or [`DEFAULT_DAYS`] capped by the whole
/// days left on the CA's own certificate.
pub fn days_or_default(ca: &Ca, given: Option<u32>) -> u32 {
    given.unwrap_or_else(|| {
        let ca_until = ca
            .certificate
            .tbs_certificate
            .validity
            .not_after
            .to_unix_duration();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let left = ca_until.saturating_sub(now).as_secs() / 86_400;
        DEFAULT_DAYS.min(u32::try_from(left).unwrap_or(u32::MAX))
    })
}

/// Sign `request_pem` with `ca` for `days` days. A validity past the CA's
/// own is refused.
pub fn sign(ca: &Ca, request_pem: &str, days: u32) -> Result<Signed, String> {
    if days == 0 {
        return Err("--days must be at least 1".into());
    }
    let (installation, key) = check_request(request_pem)?;
    let validity = Validity::from_now(Duration::from_secs(u64::from(days) * 86_400))
        .map_err(|e| e.to_string())?;
    let ca_until = ca
        .certificate
        .tbs_certificate
        .validity
        .not_after
        .to_unix_duration();
    if validity.not_after.to_unix_duration() > ca_until {
        return Err(format!(
            "{days} days goes past the CA certificate's own validity ({})",
            ca.certificate.tbs_certificate.validity.not_after
        ));
    }
    let serial = new_serial()?;
    let profile = Profile::Leaf {
        issuer: ca.subject().clone(),
        enable_key_agreement: false,
        enable_key_encipherment: false,
    };
    let fail = |e: x509_cert::builder::Error| format!("could not build the certificate: {e}");
    let mut builder = CertificateBuilder::new(
        profile,
        serial.clone(),
        validity,
        installation_subject(&installation)?,
        crate::pki::public_key_info(&key)?,
        &ca.key,
    )
    .map_err(fail)?;
    builder
        .add_extension(&ClientAuth(ExtendedKeyUsage(vec![
            rfc5280::ID_KP_CLIENT_AUTH,
        ])))
        .map_err(fail)?;
    let certificate = builder.build::<DerSignature>().map_err(fail)?;
    Ok(Signed {
        installation,
        serial: serial_hex(&serial),
        certificate_pem: certificate
            .to_pem(LineEnding::LF)
            .map_err(|e| e.to_string())?,
    })
}

/// Extended key usage `clientAuth`, not critical, as the certificates
/// Roles Anywhere accepted in `tools/verify/roles-anywhere.sh` carry it.
struct ClientAuth(ExtendedKeyUsage);

impl AssociatedOid for ClientAuth {
    const OID: ObjectIdentifier = rfc5280::ID_CE_EXT_KEY_USAGE;
}

impl Encode for ClientAuth {
    fn encoded_len(&self) -> x509_cert::der::Result<Length> {
        self.0.encoded_len()
    }

    fn encode(&self, writer: &mut impl Writer) -> x509_cert::der::Result<()> {
        self.0.encode(writer)
    }
}

impl AsExtension for ClientAuth {
    fn critical(&self, _: &Name, _: &[Extension]) -> bool {
        false
    }
}
