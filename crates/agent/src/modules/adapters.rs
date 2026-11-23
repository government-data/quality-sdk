//! The [`ModuleHost`] of an installation: the signed index and the registry
//! (`AwsCloud`), the package files, Docker Compose, the state file, and
//! quality-data's `updater/v1` to tell it the modules.

use std::path::{Path, PathBuf};

use module_manifest::Manifest;
use updater::engine::{CloudError, Installed, Offer, OfferedPackage};
use updater::state::{Modules, Phase, State};

use super::ModuleHost;
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

/// Refuse while an update runs; a prepared version goes back to idle
/// (written to `state_file`), to be prepared again with the modules
/// installed now.
pub fn no_update_running(state_file: &Path) -> Result<(), String> {
    let Some(mut s) = State::load(state_file)? else {
        return Ok(());
    };
    match s.phase {
        Phase::Idle => Ok(()),
        Phase::Prepared { .. } => {
            s.phase = Phase::Idle;
            s.save(state_file)
        }
        _ => Err(format!(
            "an update is under way ({}): run it again once it ended",
            s.phase.name()
        )),
    }
}

/// The `module.yaml` of a package, extracted into `scratch` (emptied
/// before and after).
pub fn package_manifest(package: &Path, scratch: &Path) -> Result<Option<Manifest>, String> {
    let _ = std::fs::remove_dir_all(scratch);
    let read = crate::package_archive::extract(package, scratch).and_then(|()| {
        let path = scratch.join(crate::manifests::FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Manifest::parse(&text)
                .map(Some)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    });
    let _ = std::fs::remove_dir_all(scratch);
    read
}

impl ModuleHost for Adapters {
    fn no_update_running(&self) -> Result<(), String> {
        no_update_running(&self.paths.state_file())
    }
    fn installed(&self) -> Result<(Installed, Vec<String>), String> {
        Ok((self.compose.installed()?, self.compose.order()?))
    }
    fn manifest(&self, module: &str) -> Result<Option<Manifest>, String> {
        crate::manifests::read(self.paths.install_dir(), module)
    }
    fn offer(&self, version: &str) -> Result<Offer, String> {
        self.cloud.choose(Some(version)).map_err(|e| match e {
            CloudError::Rejected(m) | CloudError::Network(m) => m,
        })
    }
    fn download(&self, offer: &Offer, package: &OfferedPackage) -> Result<PathBuf, String> {
        self.cloud.download_package(offer, package)
    }
    fn package_manifest(&self, package: &Path) -> Result<Option<Manifest>, String> {
        let scratch = self.paths.data_dir().join("updater/module-check");
        package_manifest(package, &scratch)
    }
    fn pull_images(&self, offer: &Offer) -> Result<(), String> {
        self.cloud.pull_images(offer)
    }
    fn put_package(&self, module: &str, package: &Path) -> Result<(), String> {
        self.packages.put(module, package, &[])
    }
    fn put_modules(
        &self,
        version: &str,
        modules: &Modules,
        order: &[String],
    ) -> Result<(), String> {
        Compose::put_version(&self.compose, version, modules, order)
    }
    fn up(&self, module: &str) -> Result<(), String> {
        self.compose.up_one(module)
    }
    fn down(&self, module: &str) -> Result<(), String> {
        self.compose.down_one(module)
    }
    fn healthy(&self, module: &str) -> Result<(), String> {
        self.compose.healthy_one(module)
    }
    fn routes(&self) -> Result<(), String> {
        self.compose.sync_routes()
    }
    fn tell_core(&self) -> Result<(), String> {
        crate::service::assemble(&self.paths)?.put_modules()
    }
}

#[cfg(test)]
#[path = "adapters_tests.rs"]
mod tests;
