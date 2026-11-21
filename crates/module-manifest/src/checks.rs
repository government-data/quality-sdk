//! The two checks between installed modules (decision 0002): whether they
//! meet a module's requirements, and which would lose a required
//! capability if one module were removed.

use crate::model::{Manifest, RequiredCapability};

/// Why a requirement is not met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// No installed module provides the capability.
    NoProvider,
    /// It is provided, at other versions only.
    OtherVersions(Vec<u32>),
    /// It is provided at that version without these protocols (by the
    /// provider that misses the fewest).
    MissingProtocols(Vec<String>),
}

/// A requirement of a module that the installed modules do not meet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmet {
    pub capability: String,
    pub version: u32,
    /// An optional requirement warns; a required one refuses.
    pub optional: bool,
    pub why: Why,
}

/// A module that would lose a capability it requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loss {
    pub module: String,
    pub capability: String,
    pub version: u32,
    /// An optional requirement warns; a required one refuses the removal.
    pub optional: bool,
}

/// Why `required` is not met by `providers`, or `None` when it is: one of
/// them provides that name, at that version, with every protocol listed.
fn why_unmet<'a>(
    required: &RequiredCapability,
    providers: impl Iterator<Item = &'a Manifest>,
) -> Option<Why> {
    let mut versions = Vec::new();
    let mut fewest: Option<Vec<String>> = None;
    for c in providers
        .flat_map(|m| &m.provides.capabilities)
        .filter(|c| c.name == required.name)
    {
        if c.version != required.version {
            versions.push(c.version);
            continue;
        }
        let missing: Vec<String> = required
            .protocols
            .iter()
            .filter(|p| !c.protocols.contains(p))
            .cloned()
            .collect();
        if missing.is_empty() {
            return None;
        }
        if fewest.as_ref().is_none_or(|f| missing.len() < f.len()) {
            fewest = Some(missing);
        }
    }
    if let Some(missing) = fewest {
        return Some(Why::MissingProtocols(missing));
    }
    if versions.is_empty() {
        return Some(Why::NoProvider);
    }
    versions.sort_unstable();
    versions.dedup();
    Some(Why::OtherVersions(versions))
}

/// The requirements of `module` that `installed` does not meet (`module`
/// itself, if listed, is not its own provider). Empty: every one is met.
pub fn unmet(module: &Manifest, installed: &[Manifest]) -> Vec<Unmet> {
    module
        .requires
        .capabilities
        .iter()
        .filter_map(|r| {
            let others = installed.iter().filter(|m| m.name != module.name);
            why_unmet(r, others).map(|why| Unmet {
                capability: r.name.clone(),
                version: r.version,
                optional: r.optional,
                why,
            })
        })
        .collect()
}

/// What the installed modules would lose if `removed` were removed: every
/// requirement met now that no other installed module would meet. A
/// required one refuses the removal; an optional one warns.
pub fn lost_on_removal(removed: &str, installed: &[Manifest]) -> Vec<Loss> {
    let mut lost = Vec::new();
    for m in installed.iter().filter(|m| m.name != removed) {
        for r in &m.requires.capabilities {
            let all = installed.iter().filter(|p| p.name != m.name);
            let rest = all.clone().filter(|p| p.name != removed);
            if why_unmet(r, all).is_none() && why_unmet(r, rest).is_some() {
                lost.push(Loss {
                    module: m.name.clone(),
                    capability: r.name.clone(),
                    version: r.version,
                    optional: r.optional,
                });
            }
        }
    }
    lost
}
