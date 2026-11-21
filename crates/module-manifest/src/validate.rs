//! The rules every manifest keeps.

use crate::model::Manifest;
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

impl Manifest {
    pub(crate) fn validate(&self) -> Result<(), ManifestError> {
        check_name("the module", &self.name)?;
        for c in &self.provides.capabilities {
            if let Some(h) = &c.hold {
                if duration_secs(&h.ttl).is_none() {
                    return Err(invalid(format!(
                        "provides {}: hold.ttl {:?} is not <n>s|m|h",
                        c.name, h.ttl
                    )));
                }
            }
        }
        Ok(())
    }
}
