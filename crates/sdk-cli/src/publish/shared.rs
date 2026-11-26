//! A module that requires a capability at its exact version runs beside
//! its provider, so an image both mirror must be the same image: decision
//! 0011's Trino lockstep (`query-trino` requires `trino-cluster` `484` of
//! `engine-trino`; both mirror the same Trino digest), generalised as
//! ruled on 2026-11-25. For each requirement a module's manifest declares,
//! each module of the version whose manifest provides that capability at
//! that version, and each image name both modules give in the version, the
//! digests must be equal; otherwise `publish` refuses before writing
//! anything.

use std::collections::BTreeMap;

use module_manifest::Manifest;

/// The digest of `image` (`<repository>@sha256:<hex>`), or the whole text.
fn digest(image: &str) -> &str {
    image.split_once('@').map_or(image, |(_, d)| d)
}

/// `images`: each module's images by name, as the version names them;
/// `manifests`: each module's manifest (a module without one is skipped).
/// The error names every pair that differs.
pub fn check(
    version: &str,
    images: &BTreeMap<String, BTreeMap<String, String>>,
    manifests: &BTreeMap<String, Manifest>,
) -> Result<(), String> {
    let mut wrong = Vec::new();
    for (module, manifest) in manifests {
        for required in &manifest.requires.capabilities {
            let providers = manifests.iter().filter(|(other, m)| {
                *other != module
                    && m.provides
                        .capabilities
                        .iter()
                        .any(|p| p.name == required.name && p.version == required.version)
            });
            for (provider, _) in providers {
                let (Some(mine), Some(theirs)) = (images.get(module), images.get(provider)) else {
                    continue;
                };
                for (name, image) in mine {
                    let Some(other) = theirs.get(name) else {
                        continue;
                    };
                    if digest(image) != digest(other) {
                        wrong.push(format!(
                            "{module} requires {} {} of {provider}, and mirrors {name} at {} \
                             where {provider} mirrors {}",
                            required.name,
                            required.version,
                            digest(image),
                            digest(other)
                        ));
                    }
                }
            }
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{version} cannot be published: an image shared with a provider must be the same \
             digest: {}",
            wrong.join("; ")
        ))
    }
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;
