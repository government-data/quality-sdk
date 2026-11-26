//! A module's data (decision 0002): what a backup covers.

use serde::Deserialize;

/// A module's data. A critical piece is backed up before every update; one
/// that is not (a cache) is kept on remove but not backed up.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    #[serde(default)]
    pub databases: Vec<Database>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub keys: Vec<Key>,
}

/// A database of the installation's PostgreSQL.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    pub name: String,
    /// The agent creates it, with the module's role (decision 0009).
    #[serde(default)]
    pub create: bool,
    #[serde(default = "critical")]
    pub critical: bool,
}

/// A Compose volume, read through a service whose container mounts it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Volume {
    /// Names it in the backup.
    pub name: String,
    pub service: String,
    /// Where it is mounted in that service (absolute).
    pub path: String,
    #[serde(default = "critical")]
    pub critical: bool,
    /// How a critical volume is backed up before an update, as written:
    /// `None` when the key is left out ([`Volume::backup`] reads it).
    #[serde(default, rename = "backup")]
    written_backup: Option<Backup>,
}

impl Volume {
    /// How it is backed up: as written, [`Backup::Dump`] when left out.
    pub fn backup(&self) -> Backup {
        self.written_backup.unwrap_or_default()
    }

    /// Whether the manifest writes `backup`, `dump` or `replica`: a key an
    /// agent older than [`crate::REPLICA_BACKUP_AGENT`] refuses.
    pub fn backup_written(&self) -> bool {
        self.written_backup.is_some()
    }
}

/// How a critical volume is backed up (decision 0002).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backup {
    /// The agent copies it into the backup and checks the copy.
    #[default]
    Dump,
    /// A replica in sync is its backup: the agent does not copy it, and
    /// refuses the update unless the core says the replica is in sync.
    Replica,
}

/// A file under the module's directory (`/opt/quality-data/<module>/`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    /// Relative to the module's directory.
    pub path: String,
    #[serde(default = "critical")]
    pub critical: bool,
}

fn critical() -> bool {
    true
}

impl Data {
    /// The critical pieces: what a backup must cover.
    pub fn critical(&self) -> Data {
        Data {
            databases: self
                .databases
                .iter()
                .filter(|d| d.critical)
                .cloned()
                .collect(),
            volumes: self
                .volumes
                .iter()
                .filter(|v| v.critical)
                .cloned()
                .collect(),
            keys: self.keys.iter().filter(|k| k.critical).cloned().collect(),
        }
    }

    /// The critical pieces the agent copies into a backup: every critical
    /// one but the volumes whose backup is a replica.
    pub fn dumped(&self) -> Data {
        let mut data = self.critical();
        data.volumes.retain(|v| v.backup() == Backup::Dump);
        data
    }

    /// The critical volumes whose backup is a replica in sync.
    pub fn replicated(&self) -> impl Iterator<Item = &Volume> {
        self.volumes
            .iter()
            .filter(|v| v.critical && v.backup() == Backup::Replica)
    }
}
