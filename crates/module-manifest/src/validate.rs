//! The rules every manifest keeps.

use std::collections::BTreeSet;

use crate::model::{Backup, Data, Drain, Health, Manifest, MAX_CHECK_TIMEOUT_SECS};
use crate::ManifestError;

fn invalid(why: String) -> ManifestError {
    ManifestError::Invalid(why)
}

/// `[a-z0-9]` then `[a-z0-9_-]`, at most 63: a module, a capability, a
/// volume's name in the backup.
fn check_name(what: &str, name: &str) -> Result<(), ManifestError> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'));
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "{what} {name:?}: lowercase letters, digits, '-' or '_'"
        )))
    }
}

/// A protocol: as a name, with `/` and `.` (`lake-admin/1`).
fn check_protocol(what: &str, p: &str) -> Result<(), ManifestError> {
    let ok = !p.is_empty()
        && p.len() <= 63
        && p.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '/' | '.')
        });
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "{what}: the protocol {p:?} cannot be named"
        )))
    }
}

/// A Compose service: letters, digits, `.`, `_` and `-`.
pub(crate) fn check_service(what: &str, s: &str) -> Result<(), ManifestError> {
    let ok = !s.is_empty()
        && s.len() <= 63
        && s.starts_with(|c: char| c.is_ascii_alphanumeric())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(invalid(format!("{what}: {s:?} is not a Compose service")))
    }
}

/// Each `item` of `what` once.
fn check_unique<'a>(
    what: &str,
    items: impl IntoIterator<Item = &'a str>,
) -> Result<(), ManifestError> {
    let mut seen = BTreeSet::new();
    for i in items {
        if !seen.insert(i) {
            return Err(invalid(format!("{what}: {i:?} appears twice")));
        }
    }
    Ok(())
}

/// `<n>s`, `<n>m` or `<n>h`, `n` above 0, in seconds.
pub(crate) fn duration_secs(text: &str) -> Option<u64> {
    let unit = match text.chars().last()? {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        _ => return None,
    };
    let n: u64 = text[..text.len() - 1].parse().ok()?;
    (n > 0).then(|| n.checked_mul(unit)).flatten()
}

/// Names no module may take: ECR's `<project>/tools/*` holds the images
/// the agent itself runs (decision 0002).
pub const RESERVED_MODULE_NAMES: &[&str] = &["tools"];

impl Manifest {
    pub(crate) fn validate(&self) -> Result<(), ManifestError> {
        check_name("the module", &self.name)?;
        if RESERVED_MODULE_NAMES.contains(&self.name.as_str()) {
            return Err(invalid(format!(
                "the module {:?}: the name is reserved for the agent's helper images",
                self.name
            )));
        }
        self.validate_capabilities()?;
        validate_health(&self.health)?;
        validate_data(&self.data)?;
        validate_drain(self.drain.as_ref())?;
        crate::route::validate(self.route.as_ref(), self.auth, self.menu.as_ref())?;
        crate::secrets::validate_secrets(&self.secrets)?;
        crate::secrets::validate_created(&self.name, &self.data.databases)?;
        let declared = crate::permissions::validate(&self.name, self.permissions.as_ref())?;
        if let Some(action) = self.menu.as_ref().and_then(|m| m.requires.as_deref()) {
            declared.check("menu.requires", action)?;
        }
        let required: Vec<&str> = self
            .requires
            .capabilities
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        crate::oidc::validate(
            self.route.as_ref(),
            self.auth == Some(crate::route::Auth::Oidc),
            self.oidc.as_ref(),
            &required,
            &declared,
        )
    }

    fn validate_capabilities(&self) -> Result<(), ManifestError> {
        let provided = &self.provides.capabilities;
        for c in provided {
            let what = format!("provides {} {}", c.name, c.version);
            check_name("a capability", &c.name)?;
            if c.version == 0 {
                return Err(invalid(format!("{what}: versions start at 1")));
            }
            if c.protocols.is_empty() {
                return Err(invalid(format!("{what}: no protocol")));
            }
            for p in &c.protocols {
                check_protocol(&what, p)?;
            }
            check_unique(&what, c.protocols.iter().map(String::as_str))?;
            if let Some(h) = &c.hold {
                if duration_secs(&h.ttl).is_none() {
                    return Err(invalid(format!(
                        "{what}: hold.ttl {:?} is not <n>s|m|h",
                        h.ttl
                    )));
                }
                if h.roles.is_empty() || h.roles.iter().any(|r| check_name("", r).is_err()) {
                    return Err(invalid(format!("{what}: hold.roles must name roles")));
                }
            }
        }
        let keys: Vec<String> = provided
            .iter()
            .map(|c| format!("{} {}", c.name, c.version))
            .collect();
        check_unique("provides", keys.iter().map(String::as_str))?;
        for c in &self.requires.capabilities {
            let what = format!("requires {} {}", c.name, c.version);
            check_name("a capability", &c.name)?;
            if c.version == 0 {
                return Err(invalid(format!("{what}: versions start at 1")));
            }
            for p in &c.protocols {
                check_protocol(&what, p)?;
            }
            check_unique(&what, c.protocols.iter().map(String::as_str))?;
            if let Some(h) = &c.hold {
                if !h.path.starts_with('/') || h.path.contains(char::is_whitespace) {
                    return Err(invalid(format!(
                        "{what}: hold.path {:?} is not a path",
                        h.path
                    )));
                }
            }
        }
        let names = self.requires.capabilities.iter().map(|c| c.name.as_str());
        check_unique("requires", names)
    }
}

/// A path the core calls on the module's service: `/` first, then
/// printable ASCII with no space, query or fragment.
fn validate_drain(drain: Option<&Drain>) -> Result<(), ManifestError> {
    let Some(d) = drain else { return Ok(()) };
    let ok = d.path.starts_with('/')
        && d.path.len() <= 255
        && d.path
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, '?' | '#' | '\\'));
    if ok {
        Ok(())
    } else {
        Err(invalid(format!("drain.path {:?} is not a path", d.path)))
    }
}

fn validate_health(h: &Health) -> Result<(), ManifestError> {
    for s in &h.services {
        check_service("health.services", s)?;
    }
    check_unique("health.services", h.services.iter().map(String::as_str))?;
    for c in &h.checks {
        check_service("health.checks", &c.service)?;
        if c.command.first().is_none_or(|p| p.trim().is_empty()) {
            return Err(invalid(format!(
                "health.checks of {}: no command",
                c.service
            )));
        }
        if c.timeout_secs == 0 || c.timeout_secs > MAX_CHECK_TIMEOUT_SECS {
            return Err(invalid(format!(
                "health.checks of {}: timeout_secs is 1 to {MAX_CHECK_TIMEOUT_SECS}",
                c.service
            )));
        }
    }
    Ok(())
}

/// A relative path of plain segments only: no `..`, no `.`, no empty one.
fn plain(path: &str) -> bool {
    path.split('/')
        .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

fn validate_data(d: &Data) -> Result<(), ManifestError> {
    for db in &d.databases {
        let ok = !db.name.is_empty()
            && db.name.len() <= 63
            && db
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
        if !ok {
            return Err(invalid(format!(
                "data.databases: {:?} cannot be named",
                db.name
            )));
        }
    }
    check_unique(
        "data.databases",
        d.databases.iter().map(|x| x.name.as_str()),
    )?;
    for v in &d.volumes {
        check_name("data.volumes", &v.name)?;
        check_service("data.volumes", &v.service)?;
        if !v.path.strip_prefix('/').is_some_and(plain) {
            return Err(invalid(format!(
                "data.volumes {}: {:?} is not an absolute path",
                v.name, v.path
            )));
        }
        if v.backup() == Backup::Replica && !v.critical {
            return Err(invalid(format!(
                "data.volumes {}: backup: replica needs critical: true (a piece that is not critical has no backup)",
                v.name
            )));
        }
    }
    check_unique("data.volumes", d.volumes.iter().map(|x| x.name.as_str()))?;
    for k in &d.keys {
        if !plain(&k.path) || k.path.contains(['\\', '\n']) {
            return Err(invalid(format!(
                "data.keys: {:?} is not a path under the module's directory",
                k.path
            )));
        }
    }
    check_unique("data.keys", d.keys.iter().map(|x| x.path.as_str()))
}
