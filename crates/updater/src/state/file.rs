//! How a target and a previous version are written in the state file.
//!
//! Decision 0003 adds `modules`. For one platform version the file still
//! carries the older `api` and `web` fields, which quality-data's API reads:
//! the images `api` and `web` of the module `quality-data`. A file written by
//! the older engine (no `modules`) is read as that module with those two
//! images, so an update in progress finishes or goes back after the engine is
//! replaced. Writing `api` and `web` is removed by a later contract step.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{Module, Modules, Previous, Target};

/// The only module the older fields name.
const LEGACY_MODULE: &str = "quality-data";

#[derive(Serialize, Deserialize)]
pub(super) struct TargetFile {
    version: String,
    #[serde(default)]
    modules: Modules,
    #[serde(default)]
    api: String,
    #[serde(default)]
    web: String,
    package: PathBuf,
    #[serde(default)]
    notes: String,
}

#[derive(Serialize, Deserialize)]
pub(super) struct PreviousFile {
    version: String,
    #[serde(default)]
    modules: Modules,
    #[serde(default)]
    api: String,
    #[serde(default)]
    web: String,
    package: PathBuf,
    manifest: Vec<String>,
}

/// The older fields, from the modules.
fn legacy(modules: &Modules) -> (String, String) {
    let image = |name: &str| {
        modules
            .get(LEGACY_MODULE)
            .and_then(|m| m.images.get(name))
            .cloned()
            .unwrap_or_default()
    };
    (image("api"), image("web"))
}

/// The modules, from a file that may only have the older fields.
fn modules(version: &str, modules: Modules, api: String, web: String) -> Modules {
    if !modules.is_empty() {
        return modules;
    }
    let images: std::collections::BTreeMap<_, _> = [("api", api), ("web", web)]
        .into_iter()
        .filter(|(_, image)| !image.is_empty())
        .map(|(name, image)| (name.to_string(), image))
        .collect();
    if images.is_empty() {
        return Modules::new();
    }
    let module = Module {
        version: version.into(),
        images,
    };
    Modules::from([(LEGACY_MODULE.to_string(), module)])
}

impl From<TargetFile> for Target {
    fn from(f: TargetFile) -> Self {
        Self {
            modules: modules(&f.version, f.modules, f.api, f.web),
            version: f.version,
            package: f.package,
            notes: f.notes,
        }
    }
}

impl From<Target> for TargetFile {
    fn from(t: Target) -> Self {
        let (api, web) = legacy(&t.modules);
        Self {
            version: t.version,
            modules: t.modules,
            api,
            web,
            package: t.package,
            notes: t.notes,
        }
    }
}

impl From<PreviousFile> for Previous {
    fn from(f: PreviousFile) -> Self {
        Self {
            modules: modules(&f.version, f.modules, f.api, f.web),
            version: f.version,
            package: f.package,
            manifest: f.manifest,
        }
    }
}

impl From<Previous> for PreviousFile {
    fn from(p: Previous) -> Self {
        let (api, web) = legacy(&p.modules);
        Self {
            version: p.version,
            modules: p.modules,
            api,
            web,
            package: p.package,
            manifest: p.manifest,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::tests::modules;
    use super::super::{Phase, State};
    use super::*;

    /// What the older engine wrote halfway through an update.
    fn older_file() -> serde_json::Value {
        json!({
            "phase": "applying",
            "target": {
                "version": "1.3.0",
                "api": "r/api:1.3.0@sha256:a",
                "web": "r/web:1.3.0@sha256:b",
                "package": "/var/lib/quality-data/updater/packages/1.3.0.tar.gz",
                "notes": ""
            },
            "previous": {
                "version": "1.2.0",
                "api": "r/api:1.2.0@sha256:c",
                "web": "r/web:1.2.0@sha256:d",
                "package": "/var/lib/quality-data/updater/releases/1.2.0.tar",
                "manifest": ["compose.yml"]
            },
            "backup": "/var/lib/quality-data/backups/quality-data-1",
            "installed_version": "1.2.0"
        })
    }

    #[test]
    fn a_file_of_the_older_engine_is_read_as_the_quality_data_module() {
        let s: State = serde_json::from_value(older_file()).unwrap();
        let Phase::Applying {
            target, previous, ..
        } = &s.phase
        else {
            panic!("{:?}", s.phase);
        };
        let images = |m: &Modules| m[LEGACY_MODULE].images.clone();
        assert_eq!(target.modules.len(), 1);
        assert_eq!(target.modules[LEGACY_MODULE].version, "1.3.0");
        assert_eq!(images(&target.modules)["api"], "r/api:1.3.0@sha256:a");
        assert_eq!(images(&target.modules)["web"], "r/web:1.3.0@sha256:b");
        assert_eq!(previous.modules[LEGACY_MODULE].version, "1.2.0");
        assert_eq!(images(&previous.modules)["api"], "r/api:1.2.0@sha256:c");
        assert_eq!(images(&previous.modules)["web"], "r/web:1.2.0@sha256:d");
        assert_eq!(previous.manifest, ["compose.yml"]);
    }

    #[test]
    fn written_again_it_keeps_the_older_fields_the_api_reads() {
        let s: State = serde_json::from_value(older_file()).unwrap();
        let again = serde_json::to_value(&s).unwrap();
        let old = older_file();
        for at in ["target", "previous"] {
            assert_eq!(again[at]["api"], old[at]["api"], "{at}");
            assert_eq!(again[at]["web"], old[at]["web"], "{at}");
            assert_eq!(again[at]["version"], old[at]["version"], "{at}");
        }
        assert_eq!(serde_json::from_value::<State>(again).unwrap(), s);
    }

    #[test]
    fn every_module_is_written_with_the_older_fields_beside_them() {
        let t = Target {
            version: "1.3.0".into(),
            modules: modules("1.3.0"),
            package: "/p.tar.gz".into(),
            notes: String::new(),
        };
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["api"], "r/api:1.3.0@sha256:a");
        assert_eq!(json["web"], "r/web:1.3.0@sha256:a");
        assert_eq!(
            json["modules"]["orchestrator"]["images"]["server"],
            "r/orch:1.3.0@sha256:a"
        );
        assert_eq!(json["modules"]["orchestrator"]["version"], "1.3.0");
        // Read back, the modules win over the older fields.
        assert_eq!(serde_json::from_value::<Target>(json).unwrap(), t);
    }

    #[test]
    fn without_the_quality_data_module_the_older_fields_are_empty() {
        let mut m = modules("1.3.0");
        m.remove(LEGACY_MODULE);
        let p = Previous {
            version: "1.3.0".into(),
            modules: m,
            package: "/p.tar".into(),
            manifest: Vec::new(),
        };
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["api"], "");
        assert_eq!(json["web"], "");
        assert_eq!(serde_json::from_value::<Previous>(json).unwrap(), p);
    }
}
