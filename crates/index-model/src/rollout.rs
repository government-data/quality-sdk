//! Which version an installation is offered: the agent decides it on its own
//! from the index, with no service in between.

use semver::Version;
use sha2::{Digest, Sha256};

use crate::{Index, PlatformVersion, Rollout};

/// How an installation appears among a rollout's pilots: the SHA-256 (hex)
/// of its id, so the index, which every installation reads, names none.
pub fn installation_hash(installation: &str) -> String {
    hex::encode(Sha256::digest(installation.as_bytes()))
}

/// The installation's place in a percentage rollout, 0 to 99. It depends
/// only on the id, so the same installations go first in every rollout:
/// early adopters stay early, and a stage only ever adds installations.
pub fn bucket(installation: &str) -> u8 {
    let digest = Sha256::digest(format!("quality-data rollout:{installation}").as_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    // The remainder of a division by 100 always fits in a u8.
    (u64::from_be_bytes(first) % 100) as u8
}

impl Rollout {
    /// Whether this rollout reaches the installation.
    pub fn includes(&self, installation: &str) -> bool {
        u32::from(bucket(installation)) < u32::from(self.percent)
            || self
                .pilots
                .iter()
                .any(|p| *p == installation_hash(installation))
    }
}

impl Index {
    /// The version this installation goes to, if any: the newest one that is
    /// newer than `installed`, may be applied on it (`upgrade_from`), is not
    /// paused, and whose rollout reaches the installation. Never an older or
    /// equal version, whatever the index says.
    pub fn offer(&self, installed: &Version, installation: &str) -> Option<&PlatformVersion> {
        self.versions
            .iter()
            .filter(|v| v.version > *installed)
            .filter(|v| v.upgrade_from.matches(installed))
            .filter(|v| !v.paused)
            .filter(|v| v.rollout.includes(installation))
            .max_by(|a, b| a.version.cmp(&b.version))
    }
}
