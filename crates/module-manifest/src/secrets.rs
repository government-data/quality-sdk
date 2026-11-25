//! A module's secret files and the databases the agent creates for it
//! (decision 0009).
//!
//! ```yaml
//! secrets:
//!   uid: 1000                  # owner of the files (default 0)
//!   generate:
//!     - name: secret-key       # [a-z0-9-], at most 64
//!       bytes: 32              # 16 to 64, default 32
//!       critical: true         # default true: backed up as a key
//!     - name: signing-key
//!       kind: ed25519          # random (default) | ed25519 | p256
//! data:
//!   databases:
//!     - name: bi_superset
//!       create: true           # <module> or <module>_<suffix>, `-` as `_`
//! ```

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::model::Database;
use crate::ManifestError;

/// The files the agent issues itself: no `secrets.generate` may take them.
pub const ISSUED_SECRETS: &[&str] = &[OIDC_PEOPLE_SECRET, OIDC_MACHINE_SECRET, DATABASE_PASSWORD];
/// The people's client secret (with `oidc.people`).
pub const OIDC_PEOPLE_SECRET: &str = "oidc-people-secret";
/// The machine client's secret (with `oidc.machine`).
pub const OIDC_MACHINE_SECRET: &str = "oidc-machine-secret";
/// The module's role's password (with a `create: true` database).
pub const DATABASE_PASSWORD: &str = "database-password";
/// The most databases the agent creates for one module.
pub const MAX_CREATED_DATABASES: usize = 4;
/// A generated secret's size when the manifest does not say.
pub const SECRET_BYTES: u32 = 32;

/// The files the module asks the agent to generate, and their owner.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Secrets {
    /// The owner of every secret file of the module.
    #[serde(default)]
    pub uid: u32,
    #[serde(default)]
    pub generate: Vec<GeneratedSecret>,
}

/// A file the agent makes once and keeps: random bytes or a private key.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedSecret {
    pub name: String,
    /// What the file holds (decision 0012 step 2); left out, `random`.
    /// Kept as written: an agent older than [`crate::SECRET_KIND_AGENT`]
    /// refuses the key even when it says `random`.
    #[serde(default)]
    pub kind: Option<SecretKind>,
    /// The random bytes' size, for `random` only; left out,
    /// [`SECRET_BYTES`].
    #[serde(default)]
    pub bytes: Option<u32>,
    /// Backed up and checked before every update, as a key.
    #[serde(default = "yes")]
    pub critical: bool,
}

/// `secrets.generate[].kind`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretKind {
    /// Random bytes, base64url without padding.
    #[default]
    Random,
    /// An Ed25519 private key, PKCS#8 DER in base64.
    Ed25519,
    /// A P-256 (ECDSA) private key, PKCS#8 DER in base64.
    P256,
}

/// What the agent writes into a secret file, with no newline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretContent {
    /// `bytes` random bytes, base64url without padding.
    Random { bytes: u32 },
    /// A new Ed25519 private key, PKCS#8 in standard base64.
    Ed25519,
    /// A new P-256 private key, PKCS#8 in standard base64.
    P256,
}

impl GeneratedSecret {
    /// `kind`, `random` when left out.
    pub fn kind(&self) -> SecretKind {
        self.kind.unwrap_or_default()
    }

    /// What the agent writes for this entry.
    pub fn content(&self) -> SecretContent {
        match self.kind() {
            SecretKind::Random => SecretContent::Random {
                bytes: self.bytes.unwrap_or(SECRET_BYTES),
            },
            SecretKind::Ed25519 => SecretContent::Ed25519,
            SecretKind::P256 => SecretContent::P256,
        }
    }
}

fn yes() -> bool {
    true
}

fn invalid(why: String) -> ManifestError {
    ManifestError::Invalid(why)
}

/// `secrets.generate`: names of `[a-z0-9-]`, at most 64, unique, none the
/// agent issues; 16 to 64 bytes, and `bytes` only for `random`.
pub(crate) fn validate_secrets(s: &Secrets) -> Result<(), ManifestError> {
    let mut seen = BTreeSet::new();
    for g in &s.generate {
        let ok = (1..=64).contains(&g.name.len())
            && g.name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !ok {
            return Err(invalid(format!(
                "secrets.generate: {:?}: 1 to 64 of [a-z0-9-]",
                g.name
            )));
        }
        if ISSUED_SECRETS.contains(&g.name.as_str()) {
            return Err(invalid(format!(
                "secrets.generate: {:?} is a file the agent issues",
                g.name
            )));
        }
        if !seen.insert(g.name.as_str()) {
            return Err(invalid(format!(
                "secrets.generate: {:?} appears twice",
                g.name
            )));
        }
        match (g.kind(), g.bytes) {
            (SecretKind::Random, Some(b)) if !(16..=64).contains(&b) => {
                return Err(invalid(format!(
                    "secrets.generate {}: bytes is 16 to 64",
                    g.name
                )));
            }
            (SecretKind::Ed25519 | SecretKind::P256, Some(_)) => {
                return Err(invalid(format!(
                    "secrets.generate {}: bytes is for kind random only",
                    g.name
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

/// The module's name as a PostgreSQL name: its `-` written as `_`. The
/// module's role is named so.
pub fn database_prefix(module: &str) -> String {
    module.replace('-', "_")
}

/// The databases with `create: true`: `<prefix>` or `<prefix>_<suffix>`,
/// `[a-z0-9_]`, at most 63 bytes, at most four.
pub(crate) fn validate_created(module: &str, dbs: &[Database]) -> Result<(), ManifestError> {
    let prefix = database_prefix(module);
    let created: Vec<&Database> = dbs.iter().filter(|d| d.create).collect();
    if created.len() > MAX_CREATED_DATABASES {
        return Err(invalid(format!(
            "data.databases: at most {MAX_CREATED_DATABASES} with create: true"
        )));
    }
    for d in created {
        let own = d.name == prefix
            || d.name
                .strip_prefix(&prefix)
                .and_then(|r| r.strip_prefix('_'))
                .is_some_and(|suffix| !suffix.is_empty());
        let ok = own
            && d.name.len() <= 63
            && d.name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !ok {
            return Err(invalid(format!(
                "data.databases: {:?} with create: true is named {prefix} or \
                 {prefix}_<suffix> of [a-z0-9_]",
                d.name
            )));
        }
    }
    Ok(())
}
