//! The agent renews its own certificate for the `internal` entry point
//! (decision 0003 point 4) through the core's `POST
//! /api/internal/machines/renew`, on the same entry point and with mTLS
//! with the certificate it has, when less than [`RENEW_BEFORE`] is left.
//!
//! It sends the signing request of a new EC P-256 key, `{"csr": "<PEM>"}`
//! (the core uses only its key: subject, names and term are the CA's), and
//! gets `{certificate, ca, serial, not_after, state}`. The new certificate
//! must be for the new key; then the three files are replaced together
//! ([`InternalFiles::replace`]). Until the agent first presents it, the core
//! keeps accepting the one it renewed with, so a lost answer is renewed
//! again at the next round.

use aws_client::http::{Request, Transport};
use chrono::{DateTime, TimeDelta, Utc};
use p256::ecdsa::{DerSignature, SigningKey};
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use serde::Deserialize;
use x509_cert::builder::{Builder, RequestBuilder};
use x509_cert::der::{DecodePem, EncodePem};
use x509_cert::spki::SubjectPublicKeyInfoOwned;
use x509_cert::Certificate;

use super::internal_files::{Contents, InternalFiles};

/// Renewed when less than this is left.
pub const RENEW_BEFORE: TimeDelta = TimeDelta::days(30);

/// The core's path, after the API's base (`https://host/api`, 443).
pub const PATH: &str = "/internal/machines/renew";

/// What a round's check did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Renewal {
    /// More than [`RENEW_BEFORE`] is left.
    NotDue { not_after: DateTime<Utc> },
    /// Renewed: the new certificate's end.
    Renewed { not_after: DateTime<Utc> },
}

/// The core's answer; `serial` and `state` are not needed here.
#[derive(Deserialize)]
struct Renewed {
    certificate: String,
    ca: String,
}

/// Renews the agent's certificate through the core.
pub struct Renewer {
    base: String,
    transport: Box<dyn Transport>,
    files: InternalFiles,
}

impl std::fmt::Debug for Renewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renewer")
            .field("base", &self.base)
            .field("files", &self.files)
            .finish_non_exhaustive()
    }
}

fn public_key(key: &SigningKey) -> Result<SubjectPublicKeyInfoOwned, String> {
    SubjectPublicKeyInfoOwned::from_key(*key.verifying_key()).map_err(|e| e.to_string())
}

impl Renewer {
    /// `base` is the API's (`[api] url`); `transport` presents the files'
    /// certificate ([`super::tls::Internal`]).
    pub fn new(base: &str, transport: Box<dyn Transport>, files: InternalFiles) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            transport,
            files,
        }
    }

    /// Renew when less than [`RENEW_BEFORE`] is left at `now`.
    pub fn renew_if_due(&self, now: DateTime<Utc>) -> Result<Renewal, String> {
        self.files.finish_pending()?;
        let not_after = self.files.not_after()?;
        if not_after - now >= RENEW_BEFORE {
            return Ok(Renewal::NotDue { not_after });
        }
        let not_after = self.renew()?;
        Ok(Renewal::Renewed { not_after })
    }

    /// Renew now, however much is left (`quality-data-agent renew --force`,
    /// decision 0007): a test of the renewal, or a key that may have been
    /// exposed. The new certificate's end.
    pub fn renew_now(&self) -> Result<DateTime<Utc>, String> {
        self.files.finish_pending()?;
        self.renew()
    }

    /// The serial of the certificate in use, lowercase hex (never the
    /// certificate or the key).
    pub fn serial(&self) -> Result<String, String> {
        let what = |e: &dyn std::fmt::Display| format!("{}: {e}", self.files.certificate.display());
        let pem = std::fs::read(&self.files.certificate).map_err(|e| what(&e))?;
        let certificate = Certificate::from_pem(&pem).map_err(|e| what(&e))?;
        let bytes = certificate.tbs_certificate.serial_number.as_bytes();
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    fn renew(&self) -> Result<DateTime<Utc>, String> {
        let what = format!("POST {PATH}");
        let current = std::fs::read(&self.files.certificate)
            .map_err(|e| format!("{}: {e}", self.files.certificate.display()))?;
        let subject = Certificate::from_pem(&current)
            .map_err(|e| format!("{}: {e}", self.files.certificate.display()))?
            .tbs_certificate
            .subject;
        let key = SigningKey::random(&mut rand_core::OsRng);
        let csr = RequestBuilder::new(subject, &key)
            .and_then(|b| b.build::<DerSignature>())
            .map_err(|e| format!("could not build the certificate request: {e}"))?
            .to_pem(LineEnding::LF)
            .map_err(|e| e.to_string())?;
        let mut request = Request::new("POST", &format!("{}{PATH}", self.base));
        request.headers = vec![
            ("accept".into(), "application/json".into()),
            ("content-type".into(), "application/json".into()),
        ];
        request.body = serde_json::json!({ "csr": csr }).to_string().into_bytes();
        let r = self
            .transport
            .send(request)
            .map_err(|e| format!("{what}: quality-data does not answer: {e}"))?;
        if !(200..300).contains(&r.status) {
            let answer: serde_json::Value = serde_json::from_slice(&r.body).unwrap_or_default();
            let why: String = answer["error"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(300)
                .collect();
            return Err(format!("{what}: quality-data answered {} {why}", r.status)
                .trim_end()
                .to_string());
        }
        let renewed: Renewed = serde_json::from_slice(&r.body)
            .map_err(|e| format!("{what}: an answer that does not read: {e}"))?;
        let certificate = Certificate::from_pem(renewed.certificate.as_bytes())
            .map_err(|e| format!("{what}: its certificate does not read: {e}"))?;
        if certificate.tbs_certificate.subject_public_key_info != public_key(&key)? {
            return Err(format!("{what}: its certificate is not for the key sent"));
        }
        Certificate::from_pem(renewed.ca.as_bytes())
            .map_err(|e| format!("{what}: its CA does not read: {e}"))?;
        let not_after = super::internal_files::not_after(renewed.certificate.as_bytes())?;
        let private_key = key
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| e.to_string())?
            .to_string();
        self.files.replace(&Contents {
            ca: renewed.ca,
            certificate: renewed.certificate,
            private_key,
        })?;
        Ok(not_after)
    }
}

#[cfg(test)]
#[path = "renew_tests.rs"]
mod tests;
