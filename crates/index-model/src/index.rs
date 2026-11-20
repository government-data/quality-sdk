//! The index's model and the rules every index must keep.

use std::collections::{BTreeMap, BTreeSet};

use semver::{Version, VersionReq};
use serde::Deserialize;

use crate::IndexError;

/// The only index format this crate reads. A field that restricts who may
/// take a version comes with a new format, so an older agent refuses the
/// index rather than ignoring the restriction.
pub const FORMAT: u32 = 1;

/// A channel's index: `index/<channel>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Index {
    pub format: u32,
    pub channel: Channel,
    /// Grows with every index published on the channel. An agent refuses an
    /// index lower than the last it accepted (a replayed old index).
    pub sequence: u64,
    /// When it was published (RFC 3339): for people, not for decisions.
    pub published_at: String,
    pub versions: Vec<PlatformVersion>,
}

/// Where a version is published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Platform versions.
    Stable,
    /// Rebuilt images of the same versions: security fixes, no migrations.
    Patch,
}

/// One platform version: what goes together, and to whom.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PlatformVersion {
    pub version: Version,
    /// The installed versions it can be applied on.
    pub upgrade_from: VersionReq,
    /// Whether the previous version runs on the database this one leaves.
    /// `false` means going back is restoring the backup.
    #[serde(default = "yes")]
    pub reversible: bool,
    /// A security fix that may skip the maintenance window.
    #[serde(default)]
    pub urgent: bool,
    /// Held back for everyone (the brake, or by hand).
    #[serde(default)]
    pub paused: bool,
    pub rollout: Rollout,
    /// Every module of the platform at this version, by its name.
    pub modules: BTreeMap<String, Module>,
    #[serde(default)]
    pub notes: String,
}

fn yes() -> bool {
    true
}

/// Who gets a version: the pilots, then a share of the rest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Rollout {
    /// The pilot installations, as [`crate::installation_hash`] of their ids:
    /// the index never names an installation.
    #[serde(default)]
    pub pilots: Vec<String>,
    /// The share of the other installations, 0 to 100, by
    /// [`crate::bucket`]. 0 is pilots only; 100 is everyone.
    pub percent: u8,
}

/// A module at one version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Module {
    pub version: Version,
    /// Its images by name, each pinned by digest
    /// (`<repository>@sha256:<64 hex>`).
    pub images: BTreeMap<String, String>,
    pub package: Package,
}

/// A module's package in the release bucket.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Package {
    /// Its key, under `packages/`.
    pub key: String,
    /// Its SHA-256, 64 lowercase hex.
    pub sha256: String,
}

impl Index {
    /// Parse and validate an index's bytes. [`crate::verify`] calls this
    /// once the signature holds; never act on an index parsed without it.
    pub fn parse(bytes: &[u8]) -> Result<Self, IndexError> {
        #[derive(Deserialize)]
        struct Head {
            format: u32,
        }
        let head: Head =
            serde_json::from_slice(bytes).map_err(|e| IndexError::Malformed(e.to_string()))?;
        if head.format != FORMAT {
            return Err(IndexError::UnsupportedFormat(head.format));
        }
        let index: Index =
            serde_json::from_slice(bytes).map_err(|e| IndexError::Malformed(e.to_string()))?;
        index.validate()?;
        Ok(index)
    }

    /// Refuse an index older than the last one accepted. The same sequence
    /// is accepted: it is the same publication read again.
    pub fn check_not_older(&self, last_seen: Option<u64>) -> Result<(), IndexError> {
        match last_seen {
            Some(seen) if self.sequence < seen => Err(IndexError::Replay {
                seen,
                got: self.sequence,
            }),
            _ => Ok(()),
        }
    }

    fn validate(&self) -> Result<(), IndexError> {
        let mut seen = BTreeSet::new();
        for v in &self.versions {
            if !seen.insert(&v.version) {
                return Err(invalid(format!("version {} is listed twice", v.version)));
            }
            v.validate()?;
        }
        Ok(())
    }
}

impl PlatformVersion {
    fn validate(&self) -> Result<(), IndexError> {
        let at = &self.version;
        if self.modules.is_empty() {
            return Err(invalid(format!("{at}: no modules")));
        }
        if self.rollout.percent > 100 {
            return Err(invalid(format!("{at}: rollout over 100 %")));
        }
        if let Some(p) = self.rollout.pilots.iter().find(|p| !is_sha256(p)) {
            return Err(invalid(format!("{at}: pilot {p:?} is not a SHA-256")));
        }
        for (name, module) in &self.modules {
            module.validate(&format!("{at}, {name}"))?;
        }
        Ok(())
    }
}

impl Module {
    fn validate(&self, at: &str) -> Result<(), IndexError> {
        if self.images.is_empty() {
            return Err(invalid(format!("{at}: no images")));
        }
        for (name, image) in &self.images {
            let pinned = image
                .split_once("@sha256:")
                .is_some_and(|(repo, digest)| !repo.is_empty() && is_sha256(digest));
            if !pinned {
                return Err(invalid(format!(
                    "{at}: image {name} is not pinned by digest"
                )));
            }
        }
        let key = &self.package.key;
        if !key.starts_with("packages/") || key.split('/').any(|s| s.is_empty() || s == "..") {
            return Err(invalid(format!("{at}: package key {key:?}")));
        }
        if !is_sha256(&self.package.sha256) {
            return Err(invalid(format!("{at}: package SHA-256")));
        }
        Ok(())
    }
}

/// 64 lowercase hex characters.
pub(crate) fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn invalid(message: String) -> IndexError {
    IndexError::Invalid(message)
}
