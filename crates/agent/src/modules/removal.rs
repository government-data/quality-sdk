//! The check before a module is removed or purged (decision 0015): the
//! core asks every installed module that declares `removal`, and any
//! refusal refuses, with its reason and counts. There is no `--force`.
//!
//! A core whose `updater/v1` lacks the feature `module-removal`, or that
//! does not answer, cannot be asked: then a module that requires a
//! capability whose installed provider declares `removal` is refused (that
//! provider would have been asked); any other removal goes on as before.

use module_manifest::Manifest;

use super::ModuleHost;
use crate::host::Features;

/// Whether `module` (its manifest `m`, when there is one) may be removed,
/// with the other installed modules' manifests `installed`.
pub fn check(
    host: &dyn ModuleHost,
    module: &str,
    m: Option<&Manifest>,
    installed: &[Manifest],
) -> Result<(), String> {
    let p = host.provision();
    let why_not_asked = match p.features() {
        Ok(f) if f.has(Features::MODULE_REMOVAL) => {
            let refusals = p.removal_refusals(module).map_err(|e| {
                format!("not removed: whether {module} may go could not be asked ({e}); try again")
            })?;
            if refusals.is_empty() {
                return Ok(());
            }
            let each: Vec<String> = refusals
                .iter()
                .map(|r| {
                    let counts: Vec<String> =
                        r.counts.iter().map(|(k, v)| format!("{k} {v}")).collect();
                    if counts.is_empty() {
                        format!("{} refuses: {}", r.module, r.reason)
                    } else {
                        format!("{} refuses: {} ({})", r.module, r.reason, counts.join(", "))
                    }
                })
                .collect();
            return Err(format!("not removed: {module}: {}", each.join("; ")));
        }
        Ok(_) => format!(
            "this quality-data cannot be asked: it needs a quality-data whose updater/v1 has \
             the feature {:?}",
            Features::MODULE_REMOVAL
        ),
        Err(e) => format!("quality-data cannot be asked ({e})"),
    };
    let Some(m) = m else { return Ok(()) };
    let mut refused = Vec::new();
    for c in &m.requires.capabilities {
        for provider in installed.iter().filter(|p| p.name != module) {
            let provides = provider
                .provides
                .capabilities
                .iter()
                .any(|p| p.name == c.name);
            if provides && provider.removal.is_some() {
                refused.push(format!(
                    "{module} requires {}, whose provider {} checks removals",
                    c.name, provider.name
                ));
            }
        }
    }
    if refused.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "not removed: {}; {why_not_asked}",
            refused.join("; ")
        ))
    }
}

#[cfg(test)]
#[path = "removal_tests.rs"]
mod tests;
