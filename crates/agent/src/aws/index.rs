//! The signed index (decision 0001): read `index/<channel>.json` and its
//! `.sig` from the release bucket, trust it only when the pinned key
//! verifies it and it is not older than the last one accepted, and answer
//! the version it offers this installation as the engine's [`Offer`].

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use index_model::{Channel, Index, IndexError, IndexKey, PlatformVersion};
use semver::Version;
use updater::engine::{CloudError, Offer, OfferedPackage};
use updater::state::{Module, Modules};

use super::client::{AwsError, Bucket};

/// The channel's name, as its key in the bucket.
pub fn channel_name(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "stable",
        Channel::Patch => "patch",
    }
}

/// Reads and checks a channel's index.
#[derive(Debug)]
pub struct IndexReader {
    pub channel: Channel,
    pub key: IndexKey,
    /// Where the last accepted sequence of each channel is kept.
    pub seen: PathBuf,
}

impl IndexReader {
    /// The index, verified, of this channel, and not a replay. Its sequence
    /// is kept before it is used.
    pub fn read(&self, bucket: &Bucket) -> Result<Index, CloudError> {
        let name = channel_name(self.channel);
        let key = format!("index/{name}.json");
        let bytes = bucket.get(&key).map_err(cloud)?;
        let signature = bucket.get(&format!("{key}.sig")).map_err(cloud)?;
        let signature = String::from_utf8(signature)
            .map_err(|_| CloudError::Rejected(format!("{key}.sig is not text")))?;
        let index = index_model::verify(&bytes, &signature, &self.key).map_err(rejected)?;
        if index.channel != self.channel {
            return Err(CloudError::Rejected(format!(
                "{key} is the index of another channel"
            )));
        }
        let mut seen = load_seen(&self.seen).map_err(CloudError::Network)?;
        index
            .check_not_older(seen.get(name).copied())
            .map_err(rejected)?;
        if seen.get(name) != Some(&index.sequence) {
            seen.insert(name.into(), index.sequence);
            save_seen(&self.seen, &seen).map_err(CloudError::Network)?;
        }
        Ok(index)
    }
}

fn cloud(e: AwsError) -> CloudError {
    match e {
        AwsError::Denied(m) => CloudError::Rejected(m),
        AwsError::Unavailable(m) | AwsError::Failed(m) => CloudError::Network(m),
    }
}

fn rejected(e: IndexError) -> CloudError {
    CloudError::Rejected(e.to_string())
}

/// What the index offers `installation` at `installed`: the newest version
/// it may take, as an [`Offer`]. With none, an offer of the installed
/// version itself, which the engine takes as "up to date".
pub fn offer(index: &Index, installed: &str, installation: &str) -> Result<Offer, CloudError> {
    let current = Version::parse(installed.trim().trim_start_matches('v')).map_err(|_| {
        CloudError::Rejected(format!(
            "the installed version {installed:?} is not a version"
        ))
    })?;
    Ok(match index.offer(&current, installation) {
        Some(version) => to_offer(version),
        None => Offer {
            version: installed.into(),
            modules: Modules::new(),
            packages: Vec::new(),
            notes: String::new(),
            registry: None,
        },
    })
}

/// The version a first installation takes (decision 0004): `version`,
/// whatever its pause and rollout, or with none the newest not paused
/// whose rollout reaches `installation`. `upgrade_from` is not read:
/// nothing is installed.
pub fn first(
    index: &Index,
    version: Option<&str>,
    installation: &str,
) -> Result<Offer, CloudError> {
    let chosen = match version {
        Some(v) => {
            let wanted = Version::parse(v.trim().trim_start_matches('v'))
                .map_err(|_| CloudError::Rejected(format!("{v:?} is not a version")))?;
            index.versions.iter().find(|p| p.version == wanted)
        }
        None => index
            .versions
            .iter()
            .filter(|p| !p.paused && p.rollout.includes(installation))
            .max_by(|a, b| a.version.cmp(&b.version)),
    };
    chosen.map(to_offer).ok_or_else(|| {
        CloudError::Rejected(match version {
            Some(v) => format!("the index has no version {v}"),
            None => "the index has no version for this installation".into(),
        })
    })
}

/// A platform version as the engine's offer, with what each module
/// provides and requires, which orders its packages (decision 0003).
fn to_offer(v: &PlatformVersion) -> Offer {
    let modules = v
        .modules
        .iter()
        .map(|(name, m)| {
            let module = Module {
                version: m.version.to_string(),
                images: m.images.clone(),
            };
            (name.clone(), module)
        })
        .collect();
    let packages = v
        .modules
        .iter()
        .map(|(name, m)| OfferedPackage {
            module: name.clone(),
            key: m.package.key.clone(),
            sha256: m.package.sha256.clone(),
            provides: m.provides.clone(),
            requires: m.requires.clone(),
        })
        .collect();
    Offer {
        version: v.version.to_string(),
        modules,
        packages,
        notes: v.notes.clone(),
        registry: None,
    }
}

fn load_seen(path: &Path) -> Result<BTreeMap<String, u64>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("{} is damaged: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

/// Written to a temporary file and renamed, as the state file is.
fn save_seen(path: &Path, seen: &BTreeMap<String, u64>) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let temporary = path.with_extension("json.tmp");
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut f = std::fs::File::create(&temporary)?;
        f.write_all(&serde_json::to_vec(seen).map_err(std::io::Error::other)?)?;
        f.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(dir)?.sync_all()
    };
    write().map_err(|e| format!("could not save {}: {e}", path.display()))
}

#[cfg(test)]
#[path = "index_tests.rs"]
pub(crate) mod tests;
