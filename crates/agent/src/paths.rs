//! Where the agent keeps what it reads and writes (decision 0003):
//!
//! - `/etc/quality-data`: the installation's configuration (the agent's
//!   settings, the enrolment certificate and key, the pinned index key).
//! - `/opt/quality-data`: the installed package's files.
//! - `/var/lib/quality-data`: what changes at runtime (the state file,
//!   downloaded packages, the previous package's copy, the backups).

use std::path::{Path, PathBuf};

/// The agent's three roots and every path under them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    config: PathBuf,
    install: PathBuf,
    data: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self::system()
    }
}

impl Paths {
    /// The paths of an installation.
    pub fn system() -> Self {
        Self::under(Path::new("/"))
    }

    /// The same layout under another root: for tests and development.
    pub fn under(root: &Path) -> Self {
        Self {
            config: root.join("etc/quality-data"),
            install: root.join("opt/quality-data"),
            data: root.join("var/lib/quality-data"),
        }
    }

    /// `/etc/quality-data`.
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    /// The agent's settings: `/etc/quality-data/agent.toml`.
    pub fn config_file(&self) -> PathBuf {
        self.config.join("agent.toml")
    }

    /// The enrolment certificate (its CN is the installation's id).
    pub fn certificate(&self) -> PathBuf {
        self.config.join("enrolment/certificate.pem")
    }

    /// The enrolment certificate's private key.
    pub fn private_key(&self) -> PathBuf {
        self.config.join("enrolment/private-key.pem")
    }

    /// The enrolment's certificate request, which `qd enrol`
    /// writes for the CA's holder to sign.
    pub fn certificate_request(&self) -> PathBuf {
        self.config.join("enrolment/request.csr")
    }

    /// The installation's CA certificate, which quality-data's `internal`
    /// entry point is checked against (`updater/v1`).
    pub fn internal_ca(&self) -> PathBuf {
        self.config.join("internal/ca.pem")
    }

    /// The agent's certificate for the `internal` entry point, issued by
    /// the installation's CA.
    pub fn internal_certificate(&self) -> PathBuf {
        self.config.join("internal/certificate.pem")
    }

    /// That certificate's private key.
    pub fn internal_private_key(&self) -> PathBuf {
        self.config.join("internal/private-key.pem")
    }

    /// The index's public key, pinned at enrolment.
    pub fn index_key(&self) -> PathBuf {
        self.config.join("index-key.pem")
    }

    /// `/opt/quality-data`: the installed package's files.
    pub fn install_dir(&self) -> &Path {
        &self.install
    }

    /// The folder the core's Traefik watches (core decision 0081,
    /// `/opt/quality-data/quality-data/internal`), where the agent writes
    /// each module's route, `module-<module>.yml` (decision 0002): a file
    /// put in a subfolder after Traefik started is not read.
    pub fn routes_dir(&self) -> PathBuf {
        self.install.join("quality-data/internal")
    }

    /// `/var/lib/quality-data`.
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// The update engine's state file.
    pub fn state_file(&self) -> PathBuf {
        self.data.join("updater/state.json")
    }

    /// What is installed, as Docker Compose put it (`installed.json`).
    pub fn installed_record(&self) -> PathBuf {
        self.data.join("updater/installed.json")
    }

    /// The first installation's progress (`install.json`), while it runs.
    pub fn install_progress(&self) -> PathBuf {
        self.data.join("updater/install.json")
    }

    /// The last index sequence accepted per channel (no replay of an older
    /// index).
    pub fn index_seen(&self) -> PathBuf {
        self.data.join("updater/index-seen.json")
    }

    /// Downloaded packages, checked against their SHA-256.
    pub fn packages_dir(&self) -> PathBuf {
        self.data.join("updater/packages")
    }

    /// The copy of the previous package, to go back to it.
    pub fn releases_dir(&self) -> PathBuf {
        self.data.join("updater/releases")
    }

    /// The backups taken before applying.
    pub fn backups_dir(&self) -> PathBuf {
        self.data.join("backups")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_installation_uses_the_system_paths() {
        let p = Paths::system();
        assert_eq!(p.config_file(), Path::new("/etc/quality-data/agent.toml"));
        assert_eq!(p.install_dir(), Path::new("/opt/quality-data"));
        assert_eq!(
            p.state_file(),
            Path::new("/var/lib/quality-data/updater/state.json")
        );
        assert_eq!(
            p.packages_dir(),
            Path::new("/var/lib/quality-data/updater/packages")
        );
        assert_eq!(
            p.releases_dir(),
            Path::new("/var/lib/quality-data/updater/releases")
        );
        assert_eq!(p.backups_dir(), Path::new("/var/lib/quality-data/backups"));
        assert_eq!(
            p.internal_certificate(),
            Path::new("/etc/quality-data/internal/certificate.pem")
        );
    }

    #[test]
    fn under_a_root_the_layout_is_the_same() {
        let p = Paths::under(Path::new("/tmp/x"));
        assert_eq!(
            p.index_key(),
            Path::new("/tmp/x/etc/quality-data/index-key.pem")
        );
        assert_eq!(p.data_dir(), Path::new("/tmp/x/var/lib/quality-data"));
    }
}
