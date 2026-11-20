//! What every test of the index uses: a fixed signing key, the signature as
//! the publisher writes it, and a valid index to change.

#![allow(dead_code)]

use base64::Engine;
use index_model::IndexKey;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::{EncodePublicKey, LineEnding};
use serde_json::{json, Value};

/// A fixed key: tests sign deterministically (RFC 6979), with no randomness.
pub(crate) fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32].into()).expect("a valid scalar")
}

/// The public key as the agent pins it (PEM).
pub(crate) fn index_key(seed: u8) -> IndexKey {
    let pem = signing_key(seed)
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .expect("PEM");
    IndexKey::from_public_key_pem(&pem).expect("a P-256 key")
}

/// The `.sig` file: base64 of the DER signature over the index's bytes.
pub(crate) fn sign(seed: u8, index: &[u8]) -> String {
    let signature: Signature = signing_key(seed).sign(index);
    base64::engine::general_purpose::STANDARD.encode(signature.to_der().as_bytes())
}

pub(crate) const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A module at `version`, pinned and packaged as publishing leaves it.
pub(crate) fn module(name: &str, version: &str) -> Value {
    json!({
        "version": version,
        "images": { "api": format!("123.dkr.ecr.us-east-1.amazonaws.com/quality-data/{name}/api@sha256:{DIGEST}") },
        "package": { "key": format!("packages/{name}/{version}.tar.gz"), "sha256": DIGEST },
    })
}

/// A platform version with one module, rolled out to `percent`.
pub(crate) fn platform(version: &str, upgrade_from: &str, percent: u8) -> Value {
    json!({
        "version": version,
        "upgrade_from": upgrade_from,
        "rollout": { "percent": percent },
        "modules": { "quality-data": module("quality-data", version) },
    })
}

/// A valid stable index with `versions`.
pub(crate) fn index(sequence: u64, versions: Vec<Value>) -> Value {
    json!({
        "format": 1,
        "channel": "stable",
        "sequence": sequence,
        "published_at": "2026-11-20T00:00:00Z",
        "versions": versions,
    })
}

pub(crate) fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("JSON")
}
