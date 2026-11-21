//! What is installed, as `put_version` records it: the platform version,
//! its modules, their start order, and the version it replaced.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use updater::state::Modules;

/// A platform version as it was put: its modules and their start order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Put {
    pub version: String,
    pub modules: Modules,
    pub order: Vec<String>,
}

/// `installed.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    #[serde(flatten)]
    pub now: Put,
    /// The modules of the version it replaced that this one does not have:
    /// brought down (their data kept) when it starts.
    #[serde(default)]
    pub stopping: Vec<String>,
    #[serde(default)]
    pub previous: Option<Put>,
}

/// The record after `version` is put over `old`: `version` starts in
/// `order` (the order its packages were put), or, when it is the version
/// `old` replaced, in that version's order.
pub(super) fn next(
    old: Option<Record>,
    version: &str,
    modules: &Modules,
    order: &[String],
) -> Record {
    // Put again (a step run twice): what it replaced stays the same.
    if let Some(r) = old.as_ref().filter(|r| r.now.version == version) {
        let order = if order.is_empty() {
            r.now.order.clone()
        } else {
            order.to_vec()
        };
        return Record {
            now: put(version, modules, &order),
            ..r.clone()
        };
    }
    let back = old
        .as_ref()
        .and_then(|r| r.previous.as_ref())
        .filter(|p| p.version == version);
    let order = back.map_or(order, |p| p.order.as_slice());
    let now = put(version, modules, order);
    let stopping = old.as_ref().map_or_else(Vec::new, |r| {
        r.now
            .order
            .iter()
            .filter(|m| !modules.contains_key(*m))
            .cloned()
            .collect()
    });
    Record {
        now,
        stopping,
        previous: old.map(|r| r.now),
    }
}

/// `version` with `modules` starting in `order`: the modules it names
/// first, then any other by name.
fn put(version: &str, modules: &Modules, order: &[String]) -> Put {
    let mut seen = BTreeSet::new();
    let mut all: Vec<String> = order
        .iter()
        .filter(|m| modules.contains_key(*m) && seen.insert((*m).clone()))
        .cloned()
        .collect();
    all.extend(modules.keys().filter(|m| !seen.contains(*m)).cloned());
    Put {
        version: version.into(),
        modules: modules.clone(),
        order: all,
    }
}
