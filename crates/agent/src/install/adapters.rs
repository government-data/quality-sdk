//! The [`Installer`] made of the adapters that update an installation:
//! `AwsCloud`, `PackageFiles` and `DockerCompose` for choosing, preparing
//! and starting; the real host (with `UpdaterApi`) for verifying. The host
//! is built only then: its `updater/v1` client needs the internal
//! certificate, which the installer issues between the two phases.

use std::path::{Path, PathBuf};

use updater::engine::{CloudError, Host, Offer, OfferedPackage};
use updater::state::Modules;

use super::{Files, Installer};
use crate::aws::cloud::AwsCloud;
use crate::compose::DockerCompose;
use crate::config::Config;
use crate::host::{Cloud, Compose, Packages};
use crate::package_files::PackageFiles;
use crate::paths::Paths;

/// The installation's adapters, at `paths`.
pub struct Adapters {
    paths: Paths,
    cloud: AwsCloud,
    packages: PackageFiles,
    compose: DockerCompose,
}

impl std::fmt::Debug for Adapters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Adapters")
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

impl Adapters {
    /// From the settings and the installation's files.
    pub fn new(paths: &Paths) -> Result<Self, String> {
        let config = Config::load(&paths.config_file())?;
        Ok(Self {
            paths: paths.clone(),
            cloud: AwsCloud::from_config(&config, paths)?,
            packages: PackageFiles::new(paths),
            compose: DockerCompose::new(paths),
        })
    }
}

/// The files `install` looks at, at `paths`.
pub fn files(paths: &Paths) -> Files {
    Files {
        installed: paths.installed_record(),
        progress: paths.install_progress(),
    }
}

impl Installer for Adapters {
    fn choose(&self, version: Option<&str>) -> Result<Offer, CloudError> {
        self.cloud.choose(version)
    }
    fn download_package(&self, o: &Offer, p: &OfferedPackage) -> Result<PathBuf, String> {
        self.cloud.download_package(o, p)
    }
    fn pull_images(&self, o: &Offer) -> Result<(), String> {
        self.cloud.pull_images(o)
    }
    /// Nothing was installed: no file to remove.
    fn put_package(&self, module: &str, package: &Path) -> Result<(), String> {
        self.packages.put(module, package, &[])
    }
    fn put_version(&self, v: &str, modules: &Modules, order: &[String]) -> Result<(), String> {
        Compose::put_version(&self.compose, v, modules, order)
    }
    fn start(&self) -> Result<(), String> {
        Compose::start(&self.compose)
    }
    fn internal_certificate(&self) -> bool {
        self.paths.internal_certificate().exists()
    }
    fn healthy(&self, version: &str) -> Result<(), String> {
        crate::service::assemble(&self.paths)?.healthy(Some(version))
    }
    fn tell_modules(&self) -> Result<(), String> {
        crate::service::assemble(&self.paths)?.put_modules()
    }
    fn report(&self, version: &str, failed: Option<&str>) {
        self.cloud.report_install(version, failed);
    }
}
