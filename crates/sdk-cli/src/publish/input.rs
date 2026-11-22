//! What `publish` is given: one platform version as a TOML file, with each
//! module's package as a local file beside it. Read into the index's own
//! [`PlatformVersion`], with each package's key and SHA-256, so nothing is
//! published that the index would refuse.
//!
//! ```toml
//! version = "1.4.0"
//! upgrade_from = ">=1.3.0, <1.4.0"
//! reversible = true          # default true
//! urgent = false             # default false
//! notes = "What is new."
//!
//! [rollout]
//! percent = 10
//! pilots = ["inst-0001"]     # installation ids; the index carries their SHA-256
//!
//! [modules.quality-data]
//! version = "1.4.0"
//! package = "quality-data-1.4.0.tar.gz"   # relative to this file
//! requires = ["lake"]
//!
//! [modules.quality-data.images]
//! api = "123456789012.dkr.ecr.us-east-1.amazonaws.com/quality-data/quality-data/api@sha256:…"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use index_model::{installation_hash, Module, Package, PlatformVersion, Rollout};
use semver::{Version, VersionReq};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    version: Version,
    upgrade_from: VersionReq,
    #[serde(default = "yes")]
    reversible: bool,
    #[serde(default)]
    urgent: bool,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    notes: String,
    rollout: InputRollout,
    modules: BTreeMap<String, InputModule>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputRollout {
    percent: u8,
    #[serde(default)]
    pilots: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputModule {
    version: Version,
    package: PathBuf,
    images: BTreeMap<String, String>,
    #[serde(default)]
    provides: Vec<String>,
    #[serde(default)]
    requires: Vec<String>,
}

/// A module's package to upload: its key in the bucket and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPackage {
    pub module: String,
    pub key: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

/// A platform version to publish, and the packages it names.
#[derive(Debug)]
pub struct Release {
    pub version: PlatformVersion,
    pub packages: Vec<LocalPackage>,
}

/// Read the file at `path`; packages are read relative to its directory.
pub fn read(path: &Path) -> Result<Release, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    parse(&text, path.parent().unwrap_or(Path::new(".")))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The platform version in `text`, its packages read from under `dir`.
pub fn parse(text: &str, dir: &Path) -> Result<Release, String> {
    let input: Input = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut pilots = Vec::with_capacity(input.rollout.pilots.len());
    for id in &input.rollout.pilots {
        agent::config::check_installation(id).map_err(|e| format!("pilot: {e}"))?;
        pilots.push(installation_hash(id));
    }
    let mut modules = BTreeMap::new();
    let mut packages = Vec::new();
    for (name, m) in input.modules {
        if index_model::RESERVED_MODULE_NAMES.contains(&name.as_str()) {
            return Err(format!(
                "module {name:?}: the name is reserved for the agent's helper images"
            ));
        }
        let version = m.version.to_string();
        // A name and version the agent can keep the package under.
        agent::aws::packages::package_path(Path::new("/"), &name, &version)
            .map_err(|e| format!("module {e}"))?;
        let file = dir.join(&m.package);
        let bytes = std::fs::read(&file)
            .map_err(|e| format!("{name}: could not read {}: {e}", file.display()))?;
        let package = Package {
            key: format!("packages/{name}/{version}.tar.gz"),
            sha256: hex::encode(Sha256::digest(&bytes)),
        };
        packages.push(LocalPackage {
            module: name.clone(),
            key: package.key.clone(),
            sha256: package.sha256.clone(),
            bytes,
        });
        let module = Module {
            version: m.version,
            images: m.images,
            package,
            provides: m.provides,
            requires: m.requires,
        };
        modules.insert(name, module);
    }
    let version = PlatformVersion {
        version: input.version,
        upgrade_from: input.upgrade_from,
        reversible: input.reversible,
        urgent: input.urgent,
        paused: input.paused,
        rollout: Rollout {
            pilots,
            percent: input.rollout.percent,
        },
        modules,
        notes: input.notes,
    };
    Ok(Release { version, packages })
}
