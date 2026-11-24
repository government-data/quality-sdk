//! The installed modules' critical volumes whose backup is a replica in
//! sync (`backup: replica`, decision 0002), and what the core says of each
//! module's replica (`GET /updater/v1/modules/{name}/replica`). The agent
//! never asks the lake itself: it has no lake keys and names no module.

use module_manifest::Manifest;
use updater::engine::{Replica, ReplicaState};

use crate::host::{Api, Compose};

/// Every installed module's replicated volume, the core asked once per
/// module that has one.
pub fn of_installed(compose: &dyn Compose, api: &dyn Api) -> Result<Vec<Replica>, String> {
    let installed = compose.installed()?;
    collect(
        installed.modules.keys().map(String::as_str),
        |m| compose.manifest(m),
        |m| api.replica(m),
    )
}

fn collect<'a>(
    modules: impl IntoIterator<Item = &'a str>,
    manifest: impl Fn(&str) -> Result<Option<Manifest>, String>,
    ask: impl Fn(&str) -> Result<ReplicaState, String>,
) -> Result<Vec<Replica>, String> {
    let mut replicas = Vec::new();
    for module in modules {
        let Some(manifest) = manifest(module)? else {
            continue;
        };
        let volumes: Vec<_> = manifest.data.replicated().collect();
        if volumes.is_empty() {
            continue;
        }
        let state = ask(module)?;
        replicas.extend(volumes.into_iter().map(|v| Replica {
            module: module.into(),
            volume: v.name.clone(),
            state,
        }));
    }
    Ok(replicas)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const LAKE: &str = "name: lake-rustfs\nversion: 0.1.0\ndata:\n  volumes:\n    \
        - { name: rustfs-data, service: rustfs, path: /data, backup: replica }\n    \
        - { name: config, service: rustfs, path: /etc/rustfs }\n";

    fn manifest(module: &str) -> Result<Option<Manifest>, String> {
        Ok(match module {
            "lake-rustfs" => Some(Manifest::parse(LAKE).unwrap()),
            "docs" => Some(Manifest::parse("name: docs\nversion: 0.2.0\n").unwrap()),
            _ => None,
        })
    }

    #[test]
    fn only_a_module_with_a_replicated_volume_is_asked() {
        let asked = RefCell::new(Vec::new());
        let replicas = collect(["quality-data", "docs", "lake-rustfs"], manifest, |m| {
            asked.borrow_mut().push(m.to_string());
            Ok(ReplicaState::Behind)
        })
        .unwrap();
        assert_eq!(asked.into_inner(), ["lake-rustfs"]);
        assert_eq!(
            replicas,
            [Replica {
                module: "lake-rustfs".into(),
                volume: "rustfs-data".into(),
                state: ReplicaState::Behind,
            }]
        );
    }

    #[test]
    fn with_no_replicated_volume_nothing_is_asked() {
        let replicas = collect(["quality-data", "docs"], manifest, |_| {
            panic!("the core is not asked")
        })
        .unwrap();
        assert!(replicas.is_empty());
    }

    #[test]
    fn a_core_that_does_not_answer_is_an_error() {
        let err = collect(["lake-rustfs"], manifest, |_| {
            Err("404 no such route".into())
        })
        .unwrap_err();
        assert_eq!(err, "404 no such route");
        let err = collect(
            ["lake-rustfs"],
            |_| Err("unreadable".into()),
            |_| Ok(ReplicaState::InSync),
        )
        .unwrap_err();
        assert_eq!(err, "unreadable");
    }
}
