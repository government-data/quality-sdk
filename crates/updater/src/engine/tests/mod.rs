//! Each case of the update diagram, with a fake host that writes down what it
//! is asked and fails where told.

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};

use chrono::TimeZone;

use super::*;
use crate::policy::{Mode, Policy, Request, Window};
use crate::state::Module;

mod packages;
mod preparing;
mod recovering;
mod replicas;
mod updating;

#[derive(Default)]
struct Fake {
    now: Cell<i64>,
    /// The version the cloud offers (or the error).
    cloud: RefCell<Option<Result<Offer, CloudError>>>,
    orders: RefCell<Orders>,
    /// How many times it answers "something still runs" before emptying.
    running: Cell<u32>,
    scheduled: Cell<u64>,
    /// What fails: "package" (every one), "package:<module>", "backup",
    /// "put:<package path>", "start:1.3.0", "healthy:1.3.0",
    /// "healthy:previous", "restore"…
    fails: RefCell<HashSet<&'static str>>,
    /// The version brought up.
    up: RefCell<String>,
    calls: RefCell<Vec<String>>,
    /// Every `put_version`, with the modules it was given.
    put: RefCell<Vec<(String, Modules)>>,
    saved: RefCell<Vec<State>>,
    maintenance: Cell<bool>,
    /// What `replicas` answers, one each time it is asked; when none is
    /// left, no volume is backed up by a replica.
    replicas: RefCell<VecDeque<Result<Vec<Replica>, String>>>,
}

impl Fake {
    fn new() -> Self {
        let f = Self::default();
        f.now.set(
            Utc.with_ymd_and_hms(2026, 10, 19, 12, 0, 0)
                .unwrap()
                .timestamp(),
        );
        *f.up.borrow_mut() = "1.2.0".into();
        *f.cloud.borrow_mut() = Some(Ok(offer("1.3.0")));
        f
    }
    fn failing(self, what: &'static str) -> Self {
        self.fails.borrow_mut().insert(what);
        self
    }
    fn fails_at(&self, what: &str) -> bool {
        self.fails.borrow().contains(what)
    }
    fn note(&self, s: impl Into<String>) {
        self.calls.borrow_mut().push(s.into());
    }
    fn did(&self, s: &str) -> bool {
        self.calls.borrow().iter().any(|l| l == s)
    }
    fn at_hour(&self, hour: u32) {
        // Bogota: UTC-5.
        self.now.set(
            Utc.with_ymd_and_hms(2026, 10, 19, hour + 5, 0, 0)
                .unwrap()
                .timestamp(),
        );
    }
}

/// A platform version of two modules, one with two images.
fn modules(version: &str) -> Modules {
    let module = |images: &[(&str, &str)]| Module {
        version: version.into(),
        images: images
            .iter()
            .map(|(name, repo)| (name.to_string(), format!("{repo}:{version}@sha256:a")))
            .collect(),
    };
    Modules::from([
        (
            "quality-data".into(),
            module(&[("api", "r/api"), ("web", "r/web")]),
        ),
        ("orchestrator".into(), module(&[("server", "r/orch")])),
    ])
}

/// A module's package, providing and requiring nothing.
fn package(module: &str, version: &str) -> OfferedPackage {
    OfferedPackage {
        module: module.into(),
        key: format!("packages/{module}/{version}.tar.gz"),
        sha256: "abc".into(),
        provides: Vec::new(),
        requires: Vec::new(),
    }
}

fn offer(version: &str) -> Offer {
    let modules = modules(version);
    Offer {
        version: version.into(),
        packages: modules.keys().map(|m| package(m, version)).collect(),
        modules,
        notes: "what is new".into(),
        registry: None,
    }
}

impl Host for Fake {
    fn now(&self) -> DateTime<Utc> {
        Utc.timestamp_opt(self.now.get(), 0).unwrap()
    }
    fn wait(&self, d: Duration) {
        self.now
            .set(self.now.get() + i64::try_from(d.as_secs()).unwrap());
    }
    fn save(&self, s: &State) -> Result<(), String> {
        self.saved.borrow_mut().push(s.clone());
        Ok(())
    }
    fn check_cloud(&self, _current: &str) -> Result<Offer, CloudError> {
        self.note("cloud");
        self.cloud.borrow().clone().unwrap()
    }
    fn report_to_cloud(&self, o: &Outcome) {
        self.note(format!("report_to_cloud:{:?}", o.kind));
    }
    fn free_space(&self) -> Result<(), String> {
        if self.fails_at("space") {
            return Err("200 MB left".into());
        }
        Ok(())
    }
    fn download_package(&self, o: &Offer, p: &OfferedPackage) -> Result<PathBuf, String> {
        self.note(format!("download_package:{}:{}", p.module, o.version));
        if self.fails_at("package") || self.fails_at(&format!("package:{}", p.module)) {
            return Err("the SHA-256 does not match".into());
        }
        Ok(format!("/packages/{}-{}.tar.gz", p.module, o.version).into())
    }
    fn pull_images(&self, o: &Offer) -> Result<(), String> {
        self.note(format!("pull_images:{}", o.version));
        Ok(())
    }
    fn orders(&self) -> Orders {
        self.orders.borrow().clone()
    }
    fn maintenance(&self, reason: Option<&str>, _lasts: Duration) -> Result<(), String> {
        self.maintenance.set(reason.is_some());
        self.note(if reason.is_some() {
            "maintenance:on"
        } else {
            "maintenance:off"
        });
        Ok(())
    }
    fn drain(&self, horizon: Duration) -> Result<Drain, String> {
        if !horizon.is_zero() {
            return Ok(Drain {
                drained: true,
                scheduled_soon: self.scheduled.get(),
                ..Default::default()
            });
        }
        let left = self.running.get();
        if left > 0 {
            self.running.set(left - 1);
        }
        Ok(Drain {
            drained: left == 0,
            running: u64::from(left),
            ..Default::default()
        })
    }
    fn installed(&self) -> Result<Installed, String> {
        Ok(Installed {
            version: "1.2.0".into(),
            modules: modules("1.2.0"),
        })
    }
    fn report_replica_refused(&self, o: &Outcome) {
        self.note(format!("report_replica_refused:{}", o.detail));
    }
    fn replicas(&self) -> Result<Vec<Replica>, String> {
        self.note("replicas");
        self.replicas
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Ok(Vec::new()))
    }
    fn back_up(&self) -> Result<PathBuf, String> {
        self.note("back_up");
        if self.fails_at("backup") {
            return Err("pg_restore --list failed".into());
        }
        Ok("/backups/quality-data-1".into())
    }
    fn save_package(&self, module: &str, version: &str) -> Result<(PathBuf, Vec<String>), String> {
        self.note(format!("save_package:{module}:{version}"));
        Ok((
            format!("/releases/{module}-{version}.tar").into(),
            vec!["compose.yml".into(), "assets/app-1.2.0.js".into()],
        ))
    }
    fn manifest(&self, package: &Path) -> Result<Vec<String>, String> {
        Ok(if package.to_string_lossy().contains("1.2.0") {
            vec!["compose.yml".into(), "assets/app-1.2.0.js".into()]
        } else {
            vec!["compose.yml".into(), "assets/app-1.3.0.js".into()]
        })
    }
    fn put_package(&self, module: &str, package: &Path, remove: &[String]) -> Result<(), String> {
        self.note(format!(
            "put_package:{module}:{}:remove={}",
            package.display(),
            remove.join(",")
        ));
        if self.fails_at(&format!("put:{}", package.display())) {
            return Err("no space left on the device".into());
        }
        Ok(())
    }
    fn put_version(&self, version: &str, modules: &Modules) -> Result<(), String> {
        self.note(format!("put_version:{version}"));
        self.put
            .borrow_mut()
            .push((version.into(), modules.clone()));
        *self.up.borrow_mut() = version.into();
        Ok(())
    }
    fn provision(&self, module: &str) -> Result<(), String> {
        self.note(format!("provision:{module}"));
        if self.fails_at(&format!("provision:{module}")) {
            return Err("quality-data does not answer".into());
        }
        Ok(())
    }
    fn start(&self) -> Result<(), String> {
        let v = self.up.borrow().clone();
        self.note(format!("start:{v}"));
        if self.fails_at(&format!("start:{v}")) {
            return Err("compose up failed".into());
        }
        Ok(())
    }
    fn healthy(&self, version: Option<&str>) -> Result<(), String> {
        let v = self.up.borrow().clone();
        self.note(format!("healthy:{v}"));
        if let Some(expected) = version {
            if expected != v {
                return Err(format!("{v} answers"));
            }
        }
        let key = if version.is_some() {
            format!("healthy:{v}")
        } else {
            "healthy:previous".into()
        };
        if self.fails_at(&key) {
            return Err("/api/ready did not answer in 3 min".into());
        }
        Ok(())
    }
    fn restore_database(&self, backup: &Path) -> Result<(), String> {
        self.note(format!("restore:{}", backup.display()));
        if self.fails_at("restore") {
            return Err("pg_restore failed".into());
        }
        // Restored, the previous one starts.
        self.fails.borrow_mut().remove("healthy:previous");
        Ok(())
    }
    fn install_commands(&self) -> Result<(), String> {
        self.note("install_commands");
        Ok(())
    }
}

fn automatic() -> Orders {
    Orders {
        policy: Policy {
            mode: Mode::Automatic,
            window: Window {
                days: (1..=7).collect(),
                start: "02:00".into(),
                end: "05:00".into(),
                timezone: "America/Bogota".into(),
            },
            ..Policy::default()
        },
        request: None,
    }
}

fn run(f: &Fake, s: &mut State, who: Who) {
    advance(f, s, who).unwrap();
}

/// Whether every one of `steps` was called, in that order.
fn in_order(f: &Fake, steps: &[&str]) {
    let calls = f.calls.borrow().clone();
    let mut from = 0;
    for step in steps {
        let i = calls[from..]
            .iter()
            .position(|l| l == step)
            .unwrap_or_else(|| panic!("missing {step} (after {from}): {calls:#?}"));
        from += i + 1;
    }
}

#[test]
fn a_success_logs_at_info_and_going_back_or_failing_warns() {
    use tracing::Level;
    assert_eq!(log_level(OutcomeKind::Updated), Level::INFO);
    assert_eq!(log_level(OutcomeKind::Postponed), Level::INFO);
    assert_eq!(log_level(OutcomeKind::RolledBack), Level::WARN);
    assert_eq!(log_level(OutcomeKind::Restored), Level::WARN);
    assert_eq!(log_level(OutcomeKind::Failed), Level::WARN);
    assert_eq!(log_level(OutcomeKind::Critical), Level::ERROR);
}
