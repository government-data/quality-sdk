//! A module's events (decision 0017): the topics it publishes, each with
//! the JSON Schema file in its package that validates it, and the topics
//! it reads. The agent reads each schema and tells the core both lists;
//! this crate checks their names and that each schema is a file path
//! inside the package, not that the file is there.

use serde::de::IgnoredAny;
use serde::Deserialize;

use crate::validate::{check_name, check_unique, invalid};
use crate::ManifestError;

/// The most topics a module provides, and the most it consumes.
pub const MAX_EVENTS: usize = 32;
/// The longest topic.
pub const MAX_TOPIC_LEN: usize = 127;
/// The prefix of the core's own topics.
pub const CORE_TOPIC_PREFIX: &str = "core.";

/// A topic a module publishes, at a version, with its schema.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidedEvent {
    /// `<module>.<name>`: the publishing module's name, `.`, then `[a-z0-9_.]`.
    pub topic: String,
    pub version: u32,
    /// A JSON Schema (Draft 2020-12) file, relative to the package.
    pub schema: String,
}

/// A topic a module reads, at a version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumedEvent {
    pub topic: String,
    pub version: u32,
}

/// What a module consumes from others.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consumes {
    #[serde(default)]
    pub events: Vec<ConsumedEvent>,
    // Defined by later steps of decision 0002.
    #[serde(default)]
    apis: Option<IgnoredAny>,
}

/// A topic's shape: a module name (or `core`), `.`, then segments of
/// `[a-z0-9_]` joined by `.`; returns the publisher's part.
fn check_topic<'a>(what: &str, topic: &'a str) -> Result<&'a str, ManifestError> {
    let bad = || {
        invalid(format!(
            "{what}: the topic {topic:?} is not <module>.<name> with [a-z0-9_.] after the module"
        ))
    };
    let (publisher, name) = topic.split_once('.').ok_or_else(bad)?;
    check_name(what, publisher).map_err(|_| bad())?;
    let segment_ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    if topic.len() > MAX_TOPIC_LEN || !name.split('.').all(segment_ok) {
        return Err(bad());
    }
    Ok(publisher)
}

/// A file path inside the package: relative, no `.` or `..` segment, a
/// `.json` file.
fn check_schema_path(what: &str, path: &str) -> Result<(), ManifestError> {
    let inside = !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..");
    if inside && path.ends_with(".json") && path.len() <= 255 {
        Ok(())
    } else {
        Err(invalid(format!(
            "{what}: the schema {path:?} is not a .json file inside the package"
        )))
    }
}

fn check_count(what: &str, n: usize) -> Result<(), ManifestError> {
    if n > MAX_EVENTS {
        return Err(invalid(format!("{what}: at most {MAX_EVENTS} topics")));
    }
    Ok(())
}

fn check_version(what: &str, version: u32) -> Result<(), ManifestError> {
    if version == 0 {
        return Err(invalid(format!("{what}: versions start at 1")));
    }
    Ok(())
}

pub(crate) fn validate(
    module: &str,
    provided: &[ProvidedEvent],
    consumed: &[ConsumedEvent],
) -> Result<(), ManifestError> {
    check_count("provides.events", provided.len())?;
    check_count("consumes.events", consumed.len())?;
    for e in provided {
        let what = format!("provides.events {} {}", e.topic, e.version);
        if check_topic(&what, &e.topic)? != module || e.topic.starts_with(CORE_TOPIC_PREFIX) {
            return Err(invalid(format!(
                "{what}: a module provides only topics that start with its name and '.'"
            )));
        }
        check_version(&what, e.version)?;
        check_schema_path(&what, &e.schema)?;
    }
    for e in consumed {
        let what = format!("consumes.events {} {}", e.topic, e.version);
        check_topic(&what, &e.topic)?;
        check_version(&what, e.version)?;
    }
    let key = |t: &str, v: u32| format!("{t} {v}");
    let provided_keys: Vec<String> = provided.iter().map(|e| key(&e.topic, e.version)).collect();
    check_unique("provides.events", provided_keys.iter().map(String::as_str))?;
    let consumed_keys: Vec<String> = consumed.iter().map(|e| key(&e.topic, e.version)).collect();
    check_unique("consumes.events", consumed_keys.iter().map(String::as_str))
}
