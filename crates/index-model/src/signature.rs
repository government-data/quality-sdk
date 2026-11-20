//! The index's signature: ECDSA P-256 with SHA-256, as the KMS key of
//! decision 0001 makes it (`ECC_NIST_P256`, `ECDSA_SHA_256`).
//!
//! KMS signs a `RAW` message only up to 4 KiB, and an index can be larger:
//! publishing sends KMS the SHA-256 of the index (`MessageType=DIGEST`). The
//! signature is the same one as signing the whole index with SHA-256, so it
//! is checked here over the index's exact bytes.

use base64::Engine;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;

use crate::{Index, IndexError};

/// The index's public key, pinned in the agent at enrolment: never fetched
/// from the same place as the index.
#[derive(Debug, Clone)]
pub struct IndexKey(VerifyingKey);

impl IndexKey {
    /// From DER SubjectPublicKeyInfo, what KMS `GetPublicKey` returns.
    pub fn from_public_key_der(der: &[u8]) -> Result<Self, IndexError> {
        VerifyingKey::from_public_key_der(der)
            .map(Self)
            .map_err(|e| IndexError::BadKey(e.to_string()))
    }

    /// From PEM (`-----BEGIN PUBLIC KEY-----`), as Terraform outputs it.
    pub fn from_public_key_pem(pem: &str) -> Result<Self, IndexError> {
        VerifyingKey::from_public_key_pem(pem.trim())
            .map(Self)
            .map_err(|e| IndexError::BadKey(e.to_string()))
    }
}

/// Check `signature` (base64 of the DER signature, the content of
/// `index/<channel>.json.sig`) over the index's exact bytes with `key`, and
/// only then parse and validate the index. Nothing of an index is read before
/// its signature holds.
pub fn verify(index: &[u8], signature: &str, key: &IndexKey) -> Result<Index, IndexError> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(signature.trim())
        .map_err(|e| IndexError::MalformedSignature(e.to_string()))?;
    let signature =
        Signature::from_der(&der).map_err(|e| IndexError::MalformedSignature(e.to_string()))?;
    key.0
        .verify(index, &signature)
        .map_err(|_| IndexError::BadSignature)?;
    Index::parse(index)
}
