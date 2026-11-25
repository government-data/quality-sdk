//! Development only, for `tools/verify/module-docs.sh`: `qd module
//! add|remove` (decision 0002 step 9) with the installation's real Docker
//! Compose, package files and routes, and the signed index replaced by a
//! local package and a local image (no AWS). Never shipped: `qd module` takes
//! a module only from the signed index.
//!
//! ```text
//! module_offline --root DIR [--api URL [--connect IP:PORT]] add NAME PACKAGE.tar.gz IMAGE=REF
//! module_offline --root DIR [--api URL [--connect IP:PORT]] remove|rotate NAME
//! module_offline --root DIR [--api URL [--connect IP:PORT]] purge NAME --confirm NAME
//! ```
//!
//! Without `--api`, telling quality-data prints the list `PUT
//! /updater/v1/modules` would carry, as one JSON line on standard output,
//! and a module that needs the core (its OIDC clients, a purge) is
//! refused. With it (decision 0009, V3 and V4), quality-data's
//! `updater/v1` at URL is called with the agent's certificate for the
//! internal entry point under DIR, as `[api] url` and `connect` would: the
//! modules are put, and the clients, the purge and the audit go to it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent::api::UpdaterApi;
use agent::compose::DockerCompose;
use agent::config::ApiSettings;
use agent::host::Api;
use agent::host::{Compose, Features, InstalledModule, Packages};
use agent::modules::provisioner::Provisioner;
use agent::modules::purge::purge;
use agent::modules::rotate::rotate;
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
    /// quality-data's `updater/v1`, with `--api`.
    api: Option<UpdaterApi>,
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
        let features = match &self.api {
            Some(api) => api.features()?,
            None => Features::default(),
        };
        let installed = self.compose.installed()?;
        let mut modules = Vec::new();
        for (name, m) in &installed.modules {
            let manifest = self.manifest(name)?;
            modules.push(InstalledModule::new(
                name,
                &m.version,
                manifest.as_ref(),
                &features,
            ));
        }
        match &self.api {
            Some(api) => api.put_modules(&modules),
            None => {
                let body = serde_json::json!({ "modules": modules });
                println!("{body}");
                Ok(())
            }
        }
    }
    /// Without `--api`, a module that needs its clients is refused, and
    /// audit events wait in the outbox.
    fn provision(&self) -> &dyn Provision {
        &self.provisioner
    }
    /// No AWS: nothing is reported.
    fn report(&self, _kind: &str, _platform: &str, _module: &str, _version: &str, _detail: &str) {}
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (root, mut rest) = match args.as_slice() {
        [flag, root, rest @ ..] if flag == "--root" => (PathBuf::from(root), rest),
        _ => return usage(),
    };
    let mut settings: Option<ApiSettings> = None;
    if let [flag, url, more @ ..] = rest {
        if flag == "--api" {
            let mut s = ApiSettings {
                url: url.clone(),
                connect: None,
            };
            rest = more;
            if let [flag, addr, more @ ..] = rest {
                if flag == "--connect" {
                    let Ok(addr) = addr.parse() else {
                        return usage();
                    };
                    s.connect = Some(addr);
                    rest = more;
                }
            }
            settings = Some(s);
        }
    }
    let paths = Paths::under(&root);
    let api = |s: &ApiSettings| UpdaterApi::from_settings(s, &paths);
    let (api, for_provision) = match settings.as_ref().map(|s| (api(s), api(s))) {
        None => (None, None),
        Some((Ok(a), Ok(b))) => (Some(a), Some(b)),
        Some((Err(e), _) | (_, Err(e))) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut host = Offline {
        provisioner: Provisioner::new(&paths, for_provision, "unused"),
        api,
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
        [action, name] if action == "rotate" => rotate(&host, name),
        [action, name, flag, typed] if action == "purge" && flag == "--confirm" => {
            purge(&host, name, typed)
        }
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
        "usage: module_offline --root DIR [--api URL [--connect IP:PORT]] add NAME \
         PACKAGE.tar.gz IMAGE=REF\n       \
         module_offline --root DIR [--api URL [--connect IP:PORT]] remove|rotate NAME\n       \
         module_offline --root DIR [--api URL [--connect IP:PORT]] purge NAME --confirm NAME"
    );
    ExitCode::from(2)
}
