//! A module's own actions and managed policies, and the actions it names
//! (decision 0009): `menu.requires`, a role claim's values, a policy's
//! actions.
//!
//! ```yaml
//! menu:
//!   label: { en: Analytics, es: Análisis }
//!   requires: bi-superset:Author     # hides the entry without it
//! permissions:
//!   actions:
//!     - name: bi-superset:Author
//!       description: { en: Create charts, es: Crear gráficos }
//!   policies:
//!     - name: bi-superset:Authors
//!       description: { en: BI authors, es: Autores de BI }
//!       actions: [bi-superset:Author]
//! ```
//!
//! Module actions act on the installation as a whole (resource `*`). The
//! model checks the form `<prefix>:<Verb>` and the module's own prefix; the
//! core checks the rest (an action of its catalog).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::route::InLanguages;
use crate::ManifestError;

/// The longest description of an action or a policy.
const MAX_DESCRIPTION: usize = 200;

/// The module's own actions and managed policies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub policies: Vec<Policy>,
}

/// An action, `<module>:<Verb>`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub name: String,
    pub description: InLanguages,
}

/// A managed policy, `<module>:<Name>`, and the actions it allows.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub name: String,
    pub description: InLanguages,
    pub actions: Vec<String>,
}

fn invalid(why: String) -> ManifestError {
    ManifestError::Invalid(why)
}

/// `<prefix>:<Verb>`: the prefix as a module's name, the verb a capital
/// then letters and digits; the prefix.
fn prefix(action: &str) -> Option<&str> {
    let (prefix, verb) = action.split_once(':')?;
    let prefix_ok = !prefix.is_empty()
        && prefix.len() <= 63
        && prefix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let verb_ok = verb.len() <= 64
        && verb.starts_with(|c: char| c.is_ascii_uppercase())
        && verb.chars().all(|c| c.is_ascii_alphanumeric());
    (prefix_ok && verb_ok).then_some(prefix)
}

/// The actions a module declares, to check those it names.
#[derive(Debug)]
pub(crate) struct Declared<'a> {
    module: &'a str,
    actions: BTreeSet<&'a str>,
}

impl Declared<'_> {
    /// An action the module names: of the core's catalog (its form only)
    /// or, with the module's prefix, one it declares.
    pub(crate) fn check(&self, what: &str, action: &str) -> Result<(), ManifestError> {
        let Some(prefix) = prefix(action) else {
            return Err(invalid(format!(
                "{what}: {action:?} is not an action (<prefix>:<Verb>)"
            )));
        };
        if prefix == self.module && !self.actions.contains(action) {
            return Err(invalid(format!(
                "{what}: {action:?} is not in permissions.actions"
            )));
        }
        Ok(())
    }
}

/// `permissions`: every action and policy with the module's prefix, once,
/// described in each language; a policy allows one action at least.
pub(crate) fn validate<'a>(
    module: &'a str,
    permissions: Option<&'a Permissions>,
) -> Result<Declared<'a>, ManifestError> {
    let mut declared = Declared {
        module,
        actions: BTreeSet::new(),
    };
    let Some(p) = permissions else {
        return Ok(declared);
    };
    for a in &p.actions {
        check_own(module, "permissions.actions", &a.name, &a.description)?;
        if !declared.actions.insert(a.name.as_str()) {
            return Err(invalid(format!(
                "permissions.actions: {:?} appears twice",
                a.name
            )));
        }
    }
    let mut policies = BTreeSet::new();
    for policy in &p.policies {
        check_own(
            module,
            "permissions.policies",
            &policy.name,
            &policy.description,
        )?;
        if !policies.insert(policy.name.as_str()) {
            return Err(invalid(format!(
                "permissions.policies: {:?} appears twice",
                policy.name
            )));
        }
        if policy.actions.is_empty() {
            return Err(invalid(format!("{}: no action", policy.name)));
        }
        for a in &policy.actions {
            declared.check(&policy.name, a)?;
        }
    }
    Ok(declared)
}

/// A name with the module's prefix and its description.
fn check_own(
    module: &str,
    what: &str,
    name: &str,
    description: &InLanguages,
) -> Result<(), ManifestError> {
    match prefix(name) {
        Some(p) if p == module => {}
        Some(_) => {
            return Err(invalid(format!(
                "{what}: {name:?} does not have the module's name as prefix"
            )))
        }
        None => return Err(invalid(format!("{what}: {name:?} is not <module>:<Name>"))),
    }
    for text in [&description.en, &description.es] {
        let t = text.trim();
        if t.is_empty() || t.chars().count() > MAX_DESCRIPTION || t.chars().any(char::is_control) {
            return Err(invalid(format!(
                "{name}: description {text:?}: 1 to {MAX_DESCRIPTION} printable characters"
            )));
        }
    }
    Ok(())
}
