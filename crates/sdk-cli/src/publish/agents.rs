//! Whether the agents of a channel read every package's manifest (decision
//! 0009, step Q11): a warning, never a refusal (an older agent refuses
//! a manifest with keys it does not know, which is safe).
//!
//! - **What a package needs**: its `module.yaml`'s `agent_needed`.
//! - **The oldest agent of the channel**: each installation's newest
//!   report under `reports/<installation>/` names its `agent` and, from
//!   agent 0.2.0 on, its `channel`; a report without a channel counts for
//!   every channel. An installation that never reported is not known.
//! - **A configured minimum**: the release input's `min_agent`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use index_model::Channel;
use semver::Version;

use super::input::LocalPackage;
use super::Store;

/// The oldest agent the reports name for `channel`, and how many
/// installations' reports were read for it.
pub fn oldest_reported(
    store: &dyn Store,
    channel: Channel,
) -> Result<(Option<Version>, usize), String> {
    let name = agent::aws::index::channel_name(channel);
    // The newest report of each installation: names begin with the time.
    let mut newest: BTreeMap<String, String> = BTreeMap::new();
    for key in store.list("reports/")? {
        let Some(installation) = key
            .strip_prefix("reports/")
            .and_then(|r| r.split('/').next())
        else {
            continue;
        };
        let entry = newest.entry(installation.to_string()).or_default();
        if key > *entry {
            *entry = key;
        }
    }
    let (mut oldest, mut read) = (None::<Version>, 0);
    for key in newest.values() {
        let Some(bytes) = store.get_if_exists(key)? else {
            continue;
        };
        let Ok(report) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let channel_ok = report["channel"].as_str().is_none_or(|c| c == name);
        let agent = report["agent"]
            .as_str()
            .and_then(|a| Version::parse(a).ok());
        if let (true, Some(agent)) = (channel_ok, agent) {
            read += 1;
            if oldest.as_ref().is_none_or(|o| agent < *o) {
                oldest = Some(agent);
            }
        }
    }
    Ok((oldest, read))
}

/// A warning for each package whose manifest needs a newer agent than the
/// oldest of `reported` and `min_agent`.
pub fn warnings(
    packages: &[LocalPackage],
    reported: (Option<Version>, usize),
    min_agent: Option<&Version>,
) -> Result<Vec<String>, String> {
    let (oldest_reported, read) = reported;
    let oldest = match (oldest_reported.as_ref(), min_agent) {
        (Some(a), Some(b)) => Some(a.min(b).clone()),
        (a, b) => a.or(b).cloned(),
    };
    let Some(oldest) = oldest else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for p in packages {
        let needed = match needed(p) {
            Ok(Some(n)) => n,
            Ok(None) => continue,
            Err(e) => {
                out.push(format!(
                    "{}: its manifest could not be read ({e})",
                    p.module
                ));
                continue;
            }
        };
        if needed > oldest {
            out.push(format!(
                "{}: its manifest needs agent {needed}, and the channel has agent {oldest} \
                 ({read} installations' reports read{}); an older agent refuses it",
                p.module,
                min_agent.map_or(String::new(), |m| format!(", min_agent {m}"))
            ));
        }
    }
    Ok(out)
}

/// The agent version `p`'s manifest needs (none: any agent reads it).
fn needed(p: &LocalPackage) -> Result<Option<Version>, String> {
    Ok(manifest(p)?.and_then(|m| m.agent_needed()))
}

/// `p`'s `module.yaml` (none: its package has none).
pub fn manifest(p: &LocalPackage) -> Result<Option<module_manifest::Manifest>, String> {
    use rand_core::RngCore;
    // A folder of its own: publications in one process may read at once.
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "qsdk-manifest-{}-{:016x}",
        std::process::id(),
        rand_core::OsRng.next_u64()
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join("package.tar.gz");
    let read = std::fs::write(&file, &p.bytes)
        .map_err(|e| format!("{}: {e}", file.display()))
        .and_then(|()| agent::modules::adapters::package_manifest(&file, &dir.join("x")));
    let _ = std::fs::remove_dir_all(&dir);
    read.map_err(|e| format!("{}: {e}", p.module))
}
