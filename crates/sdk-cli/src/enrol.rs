//! `qsdk enrol`, on the installation's server (decision 0001): a
//! P-256 key made here and kept here (`enrolment/private-key.pem`, PKCS #8,
//! 0600),
//! and a certificate request for the CA's holder to sign, with OU
//! `installation` and CN the installation's id. Run again, it keeps the key
//! and writes the request again.

use agent::paths::Paths;
use p256::ecdsa::DerSignature;
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use x509_cert::builder::{Builder, RequestBuilder};
use x509_cert::der::EncodePem;

use crate::pki::{
    self, installation_subject, signing_key_from_pem, write_private, write_replacing,
};

/// What enrolling left: the request, as PEM, and whether the key is new.
#[derive(Debug)]
pub struct Enrolled {
    pub request_pem: String,
    pub new_key: bool,
}

pub fn run(paths: &Paths, installation: &str) -> Result<Enrolled, String> {
    agent::config::check_installation(installation)?;
    let key_file = paths.private_key();
    let (key, new_key) = if key_file.exists() {
        (signing_key_from_pem(&pki::read(&key_file)?)?, false)
    } else {
        let key = pki::new_key();
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
