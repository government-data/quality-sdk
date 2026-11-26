//! A module's events as `PUT /updater/v1/modules` carries them (decision
//! 0017): the topics it provides, each with its JSON Schema read from the
//! package, and the topics it consumes. A schema that cannot be read, is
//! above [`MAX_SCHEMA_BYTES`] or is not a JSON object leaves the module's
//! events out, with a warning: the core then refuses its publishing rather
//! than take part of what it declares.

use module_manifest::Manifest;
use serde::Serialize;

use crate::role_files::Package;

/// The largest schema the core takes (64 KiB).
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024;

/// A module's events as the core is told them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleEvents {
    pub provides: Vec<ToldProvided>,
    pub consumes: Vec<ToldConsumed>,
}

/// A provided topic, at a version, with its schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToldProvided {
    pub topic: String,
    pub version: u32,
    pub schema: serde_json::Value,
}

/// A consumed topic, at a version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToldConsumed {
    pub topic: String,
    pub version: u32,
}

/// The events of `manifest` (none when it declares none), their schemas
/// read from the package it was put from.
pub fn derive(module: &str, manifest: &Manifest, package: &dyn Package) -> Option<ModuleEvents> {
    if !manifest.declares_events() {
        return None;
    }
    let provides = manifest
        .provides
        .events
        .iter()
        .map(|e| {
            Ok(ToldProvided {
                topic: e.topic.clone(),
                version: e.version,
                schema: schema(&e.schema, package)?,
            })
        })
        .collect::<Result<Vec<_>, String>>();
    let provides = match provides {
        Ok(p) => p,
        Err(why) => {
            tracing::warn!(%module, "its events are not told: {why}");
            return None;
        }
    };
    let consumes = manifest
        .consumes
        .events
        .iter()
        .map(|e| ToldConsumed {
            topic: e.topic.clone(),
            version: e.version,
        })
        .collect();
    Some(ModuleEvents { provides, consumes })
}

/// The schema file at `path` in the package, as a JSON object.
fn schema(path: &str, package: &dyn Package) -> Result<serde_json::Value, String> {
    let files = package.files_under(path)?;
    let [(found, bytes)] = files.as_slice() else {
        return Err(format!("the schema {path} is not one file"));
    };
    if found != path {
        return Err(format!("the schema {path} is not one file"));
    }
    if bytes.len() > MAX_SCHEMA_BYTES {
        return Err(format!(
            "the schema {path} is above {MAX_SCHEMA_BYTES} bytes"
        ));
    }
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err(format!("the schema {path} is not a JSON object")),
        Err(e) => Err(format!("the schema {path} does not read as JSON: {e}")),
    }
}

#[cfg(test)]
#[path = "module_events_tests.rs"]
mod tests;
