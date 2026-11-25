//! `qsdk publish` (decision 0001): a platform version added to a
//! channel's signed index, run by CI through the publish role or by a
//! person.
//!
//! 1. The published index is read and must verify with the index key (none
//!    yet is sequence 0); the version must not be in it.
//! 2. The next index (sequence + 1) is built and checked by the index's own
//!    rules before anything is written, and every image it names must be
//!    in the installations' ECR registry by digest ([`images`]).
//! 3. Each module's package is uploaded, or kept when the same bytes are
//!    there; other bytes under its key are refused.
//! 4. KMS signs the SHA-256 of the index (`MessageType=DIGEST`); the
//!    signature must verify with the index key.
//! 5. The index is read again: if another publication came in between, it
//!    stops. Then `index/<channel>.json` and its `.sig` are written.
//!
//! It warns, and goes on, when a package's manifest needs a newer agent
//! than the oldest agent of the channel ([`agents`], decision 0009).
//!
//! Every step can run again: a failed publication is published again.

pub mod agents;
pub mod images;
pub mod input;
pub mod shared;

use std::collections::BTreeMap;

use aws_client::client::{AwsError, Bucket};
use aws_client::kms::Kms;
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use index_model::{Channel, Index, IndexKey, FORMAT};
use sha2::{Digest, Sha256};

use images::Registry;
use input::Release;

/// Where indexes and packages are read and written: the release bucket.
pub trait Store {
    fn get_if_exists(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> Result<(), String>;
    /// Every key under `prefix`.
    fn list(&self, prefix: &str) -> Result<Vec<String>, String>;
}

impl Store for Bucket {
    fn get_if_exists(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        Bucket::get_if_exists(self, key).map_err(|e: AwsError| e.to_string())
    }

    fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> Result<(), String> {
        Bucket::put(self, key, body, content_type).map_err(|e| e.to_string())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        Bucket::list(self, prefix).map_err(|e| e.to_string())
    }
}

/// Signs a SHA-256 digest: the DER ECDSA signature.
pub trait DigestSigner {
    fn sign_digest(&self, digest: &[u8; 32]) -> Result<Vec<u8>, String>;
}

impl DigestSigner for Kms {
    fn sign_digest(&self, digest: &[u8; 32]) -> Result<Vec<u8>, String> {
        Kms::sign_digest(self, digest).map_err(|e| e.to_string())
    }
}

/// What a publication wrote.
#[derive(Debug, PartialEq, Eq)]
pub struct Published {
    pub index_key: String,
    pub sequence: u64,
    /// Package keys uploaded, and those already there with the same bytes.
    pub uploaded: Vec<String>,
    pub kept: Vec<String>,
    /// Packages whose manifest needs a newer agent than the channel's
    /// oldest ([`agents`]).
    pub warnings: Vec<String>,
}

pub fn channel_key(channel: Channel) -> String {
    format!("index/{}.json", agent::aws::index::channel_name(channel))
}

/// The channel's published index, verified, or none yet.
fn published(store: &dyn Store, channel: Channel, key: &IndexKey) -> Result<Option<Index>, String> {
    let at = channel_key(channel);
    let Some(bytes) = store.get_if_exists(&at)? else {
        return Ok(None);
    };
    let signature = store
        .get_if_exists(&format!("{at}.sig"))?
        .ok_or(format!("{at} has no {at}.sig: it is not trusted"))?;
    let signature = String::from_utf8(signature).map_err(|_| format!("{at}.sig is not text"))?;
    let index = index_model::verify(&bytes, &signature, key)
        .map_err(|e| format!("the published {at}: {e}"))?;
    if index.channel != channel {
        return Err(format!("the published {at} is another channel's"));
    }
    Ok(Some(index))
}

/// Publish `release` on `channel` at `now`.
pub fn publish(
    store: &dyn Store,
    signer: &dyn DigestSigner,
    registry: &dyn Registry,
    key: &IndexKey,
    channel: Channel,
    release: Release,
    now: DateTime<Utc>,
) -> Result<Published, String> {
    let at = channel_key(channel);
    let current = published(store, channel, key)?;
    let (sequence, mut versions) = current.map_or((0, Vec::new()), |i| (i.sequence, i.versions));
    let version = release.version.version.clone();
    if versions.iter().any(|v| v.version == version) {
        return Err(format!("{version} is already in {at}"));
    }
    versions.push(release.version);
    let next = Index {
        format: FORMAT,
        channel,
        sequence: sequence + 1,
        published_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
        versions,
    };
    let bytes = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
    Index::parse(&bytes).map_err(|e| format!("{version} cannot be published: {e}"))?;
    let added = next.versions.last().ok_or("no version to publish")?;
    // A manifest that cannot be read is warned about below, not checked.
    let mut manifests = BTreeMap::new();
    for p in &release.packages {
        if let Ok(Some(m)) = agents::manifest(p) {
            manifests.insert(p.module.clone(), m);
        }
    }
    let shared_images = added
        .modules
        .iter()
        .map(|(name, m)| (name.clone(), m.images.clone()))
        .collect();
    shared::check(&version.to_string(), &shared_images, &manifests)?;
    images::check(registry, added)?;

    let reported = agents::oldest_reported(store, channel)?;
    let warnings = agents::warnings(&release.packages, reported, release.min_agent.as_ref())?;

    let (mut uploaded, mut kept) = (Vec::new(), Vec::new());
    for p in release.packages {
        match store.get_if_exists(&p.key)? {
            Some(there) if hex::encode(Sha256::digest(&there)) == p.sha256 => kept.push(p.key),
            Some(_) => return Err(format!("{} already holds another package", p.key)),
            None => {
                store.put(&p.key, p.bytes, "application/gzip")?;
                uploaded.push(p.key);
            }
        }
    }

    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let der = signer.sign_digest(&digest)?;
    let signature = base64::engine::general_purpose::STANDARD.encode(der);
    index_model::verify(&bytes, &signature, key).map_err(|e| {
        format!("the new index's signature does not verify with the index key: {e}")
    })?;

    if let Some(now) = published(store, channel, key)? {
        if now.sequence >= next.sequence {
            return Err(format!(
                "{at} reached sequence {} while publishing {}: publish again",
                now.sequence, next.sequence
            ));
        }
    }
    store.put(&at, bytes, "application/json")?;
    store.put(&format!("{at}.sig"), signature.into_bytes(), "text/plain")?;
    Ok(Published {
        index_key: at,
        sequence: next.sequence,
        uploaded,
        kept,
        warnings,
    })
}
