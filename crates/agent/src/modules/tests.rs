//! `add` and `remove` with a fake host: the checks of decision 0002 step 4
//! (a fake `lake` capability), and what each does in which order.

use std::cell::RefCell;
use std::collections::BTreeMap;

use updater::state::Module;

use super::*;

/// The installation: its modules and their manifests, and what the index's
/// platform version carries beyond them.
struct Fake {
    installed: RefCell<BTreeMap<String, Option<Manifest>>>,
    order: RefCell<Vec<String>>,
    /// The index's modules not installed, with their package's manifest.
    index: BTreeMap<String, Option<Manifest>>,
    calls: RefCell<Vec<String>>,
    update_running: bool,
}

fn manifest(name: &str, rest: &str) -> Option<Manifest> {
    Some(Manifest::parse(&format!("name: {name}\nversion: 1.0.0\n{rest}")).unwrap())
}

const DOCS: &str = "route: { path: /docs, service: web, port: 8080 }\nauth: forward\n\
                    menu: { label: { en: User guide, es: Guía de usuario } }\n";
const PROVIDES_LAKE: &str =
    "provides: { capabilities: [{ name: lake, version: 1, protocols: [s3] }] }\n";
const REQUIRES_LAKE: &str =
    "requires: { capabilities: [{ name: lake, version: 1, protocols: [s3] }] }\n";

impl Fake {
    fn new(installed: &[(&str, &str)]) -> Self {
        Self {
            installed: RefCell::new(
                installed
                    .iter()
                    .map(|(n, rest)| ((*n).to_string(), manifest(n, rest)))
                    .collect(),
            ),
            order: RefCell::new(installed.iter().map(|(n, _)| (*n).to_string()).collect()),
            index: BTreeMap::from([("docs".to_string(), manifest("docs", DOCS))]),
            calls: RefCell::default(),
            update_running: false,
        }
    }
    fn with_index(mut self, name: &str, rest: &str) -> Self {
        self.index.insert(name.into(), manifest(name, rest));
        self
    }
    fn note(&self, s: String) {
        self.calls.borrow_mut().push(s);
    }
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

fn module() -> Module {
    Module {
        version: "1.0.0".into(),
        images: BTreeMap::from([("web".into(), "r/web@sha256:a".into())]),
    }
}

impl ModuleHost for Fake {
    fn no_update_running(&self) -> Result<(), String> {
        if self.update_running {
            return Err("an update is under way (applying)".into());
        }
        Ok(())
    }
    fn installed(&self) -> Result<(Installed, Vec<String>), String> {
        let modules = self
            .installed
            .borrow()
            .keys()
            .map(|n| (n.clone(), module()))
            .collect();
        Ok((
            Installed {
                version: "1.4.0".into(),
                modules,
            },
            self.order.borrow().clone(),
        ))
    }
    fn manifest(&self, module: &str) -> Result<Option<Manifest>, String> {
        Ok(self.installed.borrow().get(module).cloned().flatten())
    }
    fn offer(&self, version: &str) -> Result<Offer, String> {
        self.note(format!("offer {version}"));
        let names = self
            .installed
            .borrow()
            .keys()
            .chain(self.index.keys())
            .cloned()
            .collect::<Vec<_>>();
        Ok(Offer {
            version: version.into(),
            modules: names.iter().map(|n| (n.clone(), module())).collect(),
            packages: names
                .iter()
                .map(|n| OfferedPackage {
                    module: n.clone(),
                    key: format!("packages/{n}/1.0.0.tar.gz"),
                    sha256: "abc".into(),
                    provides: Vec::new(),
                    requires: Vec::new(),
                })
                .collect(),
            notes: String::new(),
            registry: None,
        })
    }
    fn download(&self, offer: &Offer, p: &OfferedPackage) -> Result<PathBuf, String> {
        assert_eq!(offer.modules.len(), 1, "the module alone");
        self.note(format!("download {}", p.module));
        Ok(PathBuf::from(format!("/packages/{}", p.module)))
    }
    fn package_manifest(&self, package: &Path) -> Result<Option<Manifest>, String> {
        let name = package.file_name().unwrap().to_str().unwrap();
        Ok(self.index.get(name).cloned().flatten())
    }
    fn pull_images(&self, offer: &Offer) -> Result<(), String> {
        let names: Vec<_> = offer.modules.keys().cloned().collect();
        self.note(format!("pull {}", names.join(",")));
        Ok(())
    }
    fn put_package(&self, module: &str, _p: &Path) -> Result<(), String> {
        self.note(format!("put_package {module}"));
        let m = self.index.get(module).cloned().flatten();
        self.installed.borrow_mut().insert(module.into(), m);
        Ok(())
    }
    fn put_modules(
        &self,
        version: &str,
        modules: &Modules,
        order: &[String],
    ) -> Result<(), String> {
        let names: Vec<_> = modules.keys().cloned().collect();
        self.note(format!(
            "put_modules {version} {} order {}",
            names.join(","),
            order.join(",")
        ));
        self.installed
            .borrow_mut()
            .retain(|n, _| modules.contains_key(n));
        *self.order.borrow_mut() = order.to_vec();
        Ok(())
    }
    fn up(&self, module: &str) -> Result<(), String> {
        self.note(format!("up {module}"));
        Ok(())
    }
    fn down(&self, module: &str) -> Result<(), String> {
        self.note(format!("down {module}"));
        Ok(())
    }
    fn healthy(&self, module: &str) -> Result<(), String> {
        self.note(format!("healthy {module}"));
        Ok(())
    }
    fn routes(&self) -> Result<(), String> {
        self.note("routes".into());
        Ok(())
    }
    fn tell_core(&self) -> Result<(), String> {
        self.note("tell".into());
        Ok(())
    }
}

#[test]
fn docs_is_added_from_the_installed_version_and_served() {
    let f = Fake::new(&[("quality-data", "")]);
    assert_eq!(add(&f, "docs").unwrap(), Warnings::new());
    assert_eq!(
        f.calls(),
        [
            "offer 1.4.0",
            "download docs",
            "pull docs",
            "put_package docs",
            "put_modules 1.4.0 docs,quality-data order quality-data,docs",
            "up docs",
            "routes",
            "healthy docs",
            "tell",
        ]
    );
}

#[test]
fn docs_is_removed_and_its_route_goes() {
    let f = Fake::new(&[("quality-data", ""), ("docs", DOCS)]);
    assert_eq!(remove(&f, "docs").unwrap(), Warnings::new());
    assert_eq!(
        f.calls(),
        [
            "down docs",
            "put_modules 1.4.0 quality-data order quality-data",
            "routes",
            "tell",
        ]
    );
}

#[test]
fn a_module_whose_required_capability_is_missing_is_not_added() {
    let f = Fake::new(&[("quality-data", "")]).with_index("query-trino", REQUIRES_LAKE);
    let err = add(&f, "query-trino").unwrap_err();
    assert!(err.contains("not added") && err.contains("lake 1"), "{err}");
    assert_eq!(
        f.calls(),
        ["offer 1.4.0", "download query-trino"],
        "nothing put"
    );

    let f = Fake::new(&[("quality-data", ""), ("lake-rustfs", PROVIDES_LAKE)])
        .with_index("query-trino", REQUIRES_LAKE);
    assert!(add(&f, "query-trino").is_ok());
}

#[test]
fn a_missing_optional_requirement_warns() {
    let optional = "requires: { capabilities: [{ name: lake, version: 1, protocols: [s3], optional: true }] }\n";
    let f = Fake::new(&[("quality-data", "")]).with_index("observability", optional);
    let warnings = add(&f, "observability").unwrap();
    assert!(warnings[0].contains("optional"), "{warnings:?}");
}

#[test]
fn removing_the_only_provider_of_a_required_capability_is_refused() {
    let f = Fake::new(&[
        ("quality-data", REQUIRES_LAKE),
        ("lake-rustfs", PROVIDES_LAKE),
    ]);
    let err = remove(&f, "lake-rustfs").unwrap_err();
    assert!(
        err.contains("quality-data requires lake 1 from lake-rustfs"),
        "{err}"
    );
    assert!(f.calls().is_empty(), "nothing stopped");

    let f = Fake::new(&[
        ("quality-data", REQUIRES_LAKE),
        ("lake-rustfs", PROVIDES_LAKE),
        ("lake-other", PROVIDES_LAKE),
    ]);
    assert!(remove(&f, "lake-rustfs").is_ok());
    assert!(f.calls().contains(&"down lake-rustfs".to_string()));
}

#[test]
fn what_cannot_be_added_or_removed_is_refused_before_anything_changes() {
    let f = Fake::new(&[("quality-data", ""), ("docs", DOCS)]);
    assert!(add(&f, "docs").unwrap_err().contains("already installed"));
    assert!(add(&f, "grafana")
        .unwrap_err()
        .contains("has no module grafana"));
    assert!(remove(&f, "grafana").unwrap_err().contains("not installed"));
    assert!(remove(&f, "quality-data").unwrap_err().contains("the core"));
    let oidc = "route: { path: /lake, service: console, port: 9001 }\nauth: oidc\n";
    let f = Fake::new(&[("quality-data", "")]).with_index("lake-rustfs", oidc);
    assert!(add(&f, "lake-rustfs").unwrap_err().contains("OIDC"));
    let mut f = Fake::new(&[("quality-data", "")]);
    f.update_running = true;
    assert!(add(&f, "docs").unwrap_err().contains("update is under way"));
    assert!(f.calls().is_empty());
}
