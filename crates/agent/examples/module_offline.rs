//! Development only, for `tools/verify/module-docs.sh`: `qd module
//! add|remove` (decision 0002 step 9) with the installation's real Docker
//! Compose, package files and routes, and the signed index replaced by a
//! local package and a local image (no AWS). Never shipped: `qd module` takes
//! a module only from the signed index.
//!
//! ```text
//! module_offline --root DIR add NAME PACKAGE.tar.gz IMAGE=REF
//! module_offline --root DIR remove NAME
//! ```
//!
//! Telling quality-data prints the list `PUT /updater/v1/modules` would
//! carry, as one JSON line on standard output.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent::compose::DockerCompose;
use agent::host::{Compose, Features, InstalledModule, Packages};
use agent::modules::provisioner::Provisioner;
use agent::modules::{adapters, add, remove, ModuleHost, Provision};
use agent::package_files::PackageFiles;
use agent::paths::Paths;
use module_manifest::Manifest;
use updater::engine::{Installed, Offer, OfferedPackage};
use updater::state::{Module, Modules};

struct Offline {
    paths: Paths,
    compose: DockerCompose,
    packages: PackageFiles,
    provisioner: Provisioner,
    /// The module's package and its one image, for `add`.
    package: PathBuf,
    image: (String, String),
}

impl ModuleHost for Offline {
    fn no_update_running(&self) -> Result<(), String> {
        adapters::no_update_running(&self.paths.state_file())
    }
    fn installed(&self) -> Result<(Installed, Vec<String>), String> {
        Ok((self.compose.installed()?, self.compose.order()?))
    }
    fn manifest(&self, module: &str) -> Result<Option<Manifest>, String> {
        agent::manifests::read(self.paths.install_dir(), module)
    }
    /// The local package as the index's module, at the package's version.
    fn offer(&self, version: &str) -> Result<Offer, String> {
        let manifest = self
            .package_manifest(&self.package)?
            .ok_or("no module.yaml")?;
        let module = Module {
            version: manifest.version.to_string(),
            images: [self.image.clone()].into(),
        };
        Ok(Offer {
            version: version.into(),
            modules: Modules::from([(manifest.name.clone(), module)]),
            packages: vec![OfferedPackage {
                module: manifest.name,
                key: self.package.display().to_string(),
                sha256: String::new(),
                provides: Vec::new(),
                requires: Vec::new(),
            }],
            notes: String::new(),
            registry: None,
        })
    }
    fn download(&self, _o: &Offer, _p: &OfferedPackage) -> Result<PathBuf, String> {
        Ok(self.package.clone())
    }
    fn package_manifest(&self, package: &Path) -> Result<Option<Manifest>, String> {
        let scratch = self.paths.data_dir().join("updater/module-check");
        adapters::package_manifest(package, &scratch)
    }
    /// The image is already in the local Docker.
    fn pull_images(&self, _o: &Offer) -> Result<(), String> {
        Ok(())
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
        let installed = self.compose.installed()?;
        let mut modules = Vec::new();
        for (name, m) in &installed.modules {
            let manifest = self.manifest(name)?;
            modules.push(InstalledModule::new(
                name,
                &m.version,
                manifest.as_ref(),
                &Features::default(),
            ));
        }
        let body = serde_json::json!({ "modules": modules });
        println!("{body}");
        Ok(())
    }
    /// No quality-data: a module that needs its clients is refused, and
    /// audit events wait in the outbox.
    fn provision(&self) -> &dyn Provision {
        &self.provisioner
    }
    /// No AWS: nothing is reported.
    fn report(&self, _kind: &str, _platform: &str, _module: &str, _version: &str, _detail: &str) {}
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (root, rest) = match args.as_slice() {
        [flag, root, rest @ ..] if flag == "--root" => (PathBuf::from(root), rest),
        _ => return usage(),
    };
    let paths = Paths::under(&root);
    let mut host = Offline {
        provisioner: Provisioner::new(&paths, None, "unused"),
        compose: DockerCompose::new(&paths),
        packages: PackageFiles::new(&paths),
        paths,
        package: PathBuf::new(),
        image: (String::new(), String::new()),
    };
    let done = match rest {
        [action, name, package, image] if action == "add" => {
            let Some((n, r)) = image.split_once('=') else {
                return usage();
            };
            host.package = PathBuf::from(package);
            host.image = (n.to_string(), r.to_string());
            add(&host, name)
        }
        [action, name] if action == "remove" => remove(&host, name),
        _ => return usage(),
    };
    match done {
        Ok(warnings) => {
            for w in warnings {
                eprintln!("warning: {w}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: module_offline --root DIR add NAME PACKAGE.tar.gz IMAGE=REF\n       \
         module_offline --root DIR remove NAME"
    );
    ExitCode::from(2)
}
