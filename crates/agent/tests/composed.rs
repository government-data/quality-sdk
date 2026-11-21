//! The agent's host composed of fake adapters: the engine runs an update
//! end to end through it, and each `Host` method reaches its adapter.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use agent::host::{
    Adapters, AgentHost, Api, Clock, Cloud, Compose, Database, InstalledModule, Packages,
};
use agent::paths::Paths;
use chrono::{DateTime, TimeZone, Utc};
use updater::engine::{advance, CloudError, Drain, Installed, Offer, OfferedPackage, Who};
use updater::policy::Orders;
use updater::state::{Module, Modules, Outcome, OutcomeKind, Phase, State};

/// What every fake shares: what was called, and what runs.
#[derive(Default)]
struct World {
    calls: RefCell<Vec<String>>,
    /// The version up, and the one quality-data answers.
    up: RefCell<String>,
    /// The version quality-data answers, when it is not the one up.
    answers: RefCell<Option<String>>,
    api_down: Cell<bool>,
    /// The modules are unhealthy, whatever version runs.
    unhealthy: Cell<bool>,
    /// Restoring the database fails.
    restore_fails: Cell<bool>,
    now: Cell<i64>,
}

impl World {
    fn note(&self, s: impl Into<String>) {
        self.calls.borrow_mut().push(s.into());
    }
    fn did(&self, prefix: &str) -> bool {
        self.calls.borrow().iter().any(|c| c.starts_with(prefix))
    }
    fn count(&self, call: &str) -> usize {
        self.calls.borrow().iter().filter(|c| *c == call).count()
    }
}

fn modules(version: &str) -> Modules {
    let image = format!("r/qd/api@sha256:{}", "a".repeat(64));
    let module = Module {
        version: version.into(),
        images: BTreeMap::from([("api".to_string(), image)]),
    };
    Modules::from([("quality-data".to_string(), module)])
}

struct FakeCloud(Rc<World>);
impl Cloud for FakeCloud {
    fn check(&self, _installed: &str) -> Result<Offer, CloudError> {
        self.0.note("cloud:check");
        Ok(Offer {
            version: "1.3.0".into(),
            modules: modules("1.3.0"),
            packages: vec![OfferedPackage {
                module: "quality-data".into(),
                key: "packages/quality-data/1.3.0.tar.gz".into(),
                sha256: "b".repeat(64),
                provides: Vec::new(),
                requires: Vec::new(),
            }],
            notes: String::new(),
            registry: None,
        })
    }
    fn report(&self, o: &Outcome) {
        self.0.note(format!("cloud:report:{:?}", o.kind));
    }
    fn download_package(&self, _o: &Offer, p: &OfferedPackage) -> Result<PathBuf, String> {
        self.0.note(format!("cloud:download:{}", p.key));
        Ok(format!("/packages/{}-1.3.0.tar.gz", p.module).into())
    }
    fn pull_images(&self, o: &Offer) -> Result<(), String> {
        self.0.note(format!("cloud:pull:{}", o.version));
        Ok(())
    }
}

struct FakeCompose(Rc<World>);
impl Compose for FakeCompose {
    fn put_version(&self, version: &str, _m: &Modules, order: &[String]) -> Result<(), String> {
        self.0
            .note(format!("compose:put:{version}:{}", order.join(",")));
        *self.0.up.borrow_mut() = version.into();
        Ok(())
    }
    fn start(&self) -> Result<(), String> {
        self.0.note("compose:start");
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        self.0.note("compose:stop");
        Ok(())
    }
    fn installed(&self) -> Result<Installed, String> {
        let up = self.0.up.borrow().clone();
        Ok(Installed {
            modules: modules(&up),
            version: up,
        })
    }
    fn modules_healthy(&self) -> Result<(), String> {
        self.0.note("compose:healthy");
        if self.0.unhealthy.get() {
            return Err("api is unhealthy".into());
        }
        Ok(())
    }
    fn drain_path(&self, module: &str) -> Result<Option<String>, String> {
        Ok((module == "quality-data").then(|| "/internal/drain".into()))
    }
}

struct FakeDatabase(Rc<World>);
impl Database for FakeDatabase {
    fn back_up(&self) -> Result<PathBuf, String> {
        self.0.note("database:back_up");
        Ok("/var/lib/quality-data/backups/1".into())
    }
    fn restore(&self, _b: &Path) -> Result<(), String> {
        self.0.note("database:restore");
        if self.0.restore_fails.get() {
            return Err("pg_restore failed on scratch".into());
        }
        Ok(())
    }
}

struct FakePackages(Rc<World>);
impl Packages for FakePackages {
    fn free_space(&self) -> Result<(), String> {
        self.0.note("packages:space");
        Ok(())
    }
    fn save(&self, module: &str, version: &str) -> Result<(PathBuf, Vec<String>), String> {
        self.0.note(format!("packages:save:{module}:{version}"));
        Ok(("/releases/1.2.0".into(), vec!["compose.yml".into()]))
    }
    fn manifest(&self, _p: &Path) -> Result<Vec<String>, String> {
        Ok(vec!["compose.yml".into()])
    }
    fn put(&self, module: &str, p: &Path, _remove: &[String]) -> Result<(), String> {
        self.0
            .note(format!("packages:put:{module}:{}", p.display()));
        Ok(())
    }
    fn install_commands(&self) -> Result<(), String> {
        self.0.note("packages:commands");
        Ok(())
    }
}

struct FakeApi(Rc<World>);
impl Api for FakeApi {
    fn put_state(&self, s: &State) -> Result<(), String> {
        if self.0.api_down.get() {
            return Err("connection refused".into());
        }
        self.0.note(format!("api:state:{}", s.phase.name()));
        Ok(())
    }
    fn orders(&self) -> Orders {
        Orders::default()
    }
    fn maintenance(&self, reason: Option<&str>, _l: Duration) -> Result<(), String> {
        self.0.note(format!("api:maintenance:{}", reason.is_some()));
        Ok(())
    }
    fn drain(&self, _h: Duration) -> Result<Drain, String> {
        Ok(Drain {
            drained: true,
            ..Drain::default()
        })
    }
    fn put_modules(&self, modules: &[InstalledModule]) -> Result<(), String> {
        if self.0.api_down.get() {
            return Err("connection refused".into());
        }
        let names: Vec<String> = modules
            .iter()
            .map(|m| {
                let drain = m.drain_path.as_deref().unwrap_or("-");
                format!("{}@{}{drain}", m.name, m.version)
            })
            .collect();
        self.0.note(format!("api:modules:{}", names.join(",")));
        Ok(())
    }
    fn version(&self) -> Result<String, String> {
        let answers = self.0.answers.borrow().clone();
        Ok(answers.unwrap_or_else(|| self.0.up.borrow().clone()))
    }
}

struct FakeClock(Rc<World>);
impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        Utc.timestamp_opt(self.0.now.get(), 0).unwrap()
    }
    fn wait(&self, d: Duration) {
        let secs = i64::try_from(d.as_secs()).unwrap();
        self.0.now.set(self.0.now.get() + secs);
    }
}

fn host(name: &str) -> (Rc<World>, AgentHost, PathBuf) {
    let world = Rc::new(World::default());
    *world.up.borrow_mut() = "1.2.0".into();
    world.now.set(
        Utc.with_ymd_and_hms(2026, 11, 21, 3, 0, 0)
            .unwrap()
            .timestamp(),
    );
    let root = std::env::temp_dir().join(format!("agent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state_file = Paths::under(&root).state_file();
    let adapters = Adapters {
        cloud: Box::new(FakeCloud(world.clone())),
        compose: Box::new(FakeCompose(world.clone())),
        database: Box::new(FakeDatabase(world.clone())),
        packages: Box::new(FakePackages(world.clone())),
        api: Box::new(FakeApi(world.clone())),
        clock: Box::new(FakeClock(world.clone())),
    };
    (world, AgentHost::new(adapters, state_file.clone()), root)
}

#[test]
fn an_update_runs_through_every_adapter_and_its_state_is_on_disk() {
    let (world, host, root) = host("update");
    let mut s = State::new("1.2.0");
    advance(&host, &mut s, Who::Person).unwrap();
    assert_eq!(s.installed_version, "1.3.0");
    assert_eq!(s.last.as_ref().unwrap().kind, OutcomeKind::Updated);
    for call in [
        "cloud:check",
        "packages:space",
        "cloud:download:packages/quality-data/1.3.0.tar.gz",
        "cloud:pull:1.3.0",
        "api:maintenance:true",
        "database:back_up",
        "packages:save:quality-data:1.2.0",
        "packages:put:quality-data:/packages/quality-data-1.3.0.tar.gz",
        "compose:put:1.3.0:quality-data",
        "compose:start",
        "api:modules:quality-data@1.3.0/internal/drain",
        "compose:healthy",
        "packages:commands",
        "api:maintenance:false",
        "cloud:report:Updated",
        "api:state:applying",
    ] {
        assert!(world.did(call), "{call}: {:#?}", world.calls.borrow());
    }
    // The state file holds what the engine stored last.
    let stored = State::load(&Paths::under(&root).state_file()).unwrap();
    assert_eq!(stored, Some(s));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_version_that_does_not_answer_as_expected_goes_back() {
    let (world, host, root) = host("rollback");
    // quality-data keeps answering the old version.
    *world.answers.borrow_mut() = Some("1.2.0".into());
    let mut s = State::new("1.2.0");
    advance(&host, &mut s, Who::Person).unwrap();
    let last = s.last.as_ref().unwrap();
    assert_eq!(last.kind, OutcomeKind::RolledBack);
    assert!(
        last.detail.contains("1.2.0 answers, not 1.3.0"),
        "{}",
        last.detail
    );
    assert!(world.did("compose:put:1.2.0"));
    assert_eq!(s.phase, Phase::Idle);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn quality_data_is_told_the_installed_modules_at_start_after_going_back_and_until_it_hears() {
    let (world, host, root) = host("modules");
    host.tell_modules_if_untold();
    host.tell_modules_if_untold();
    assert_eq!(
        world.count("api:modules:quality-data@1.2.0/internal/drain"),
        1
    );
    // A version that does not answer goes back: told after each start.
    *world.answers.borrow_mut() = Some("1.2.0".into());
    let mut s = State::new("1.2.0");
    advance(&host, &mut s, Who::Person).unwrap();
    let calls = world.calls.borrow().clone();
    let told: Vec<&String> = calls
        .iter()
        .filter(|c| c.starts_with("api:modules"))
        .collect();
    assert_eq!(
        told,
        [
            "api:modules:quality-data@1.2.0/internal/drain",
            "api:modules:quality-data@1.3.0/internal/drain",
            "api:modules:quality-data@1.2.0/internal/drain",
        ]
    );
    // Not heard: told again before the next round, until it is.
    world.api_down.set(true);
    host.tell_modules();
    world.api_down.set(false);
    world.calls.borrow_mut().clear();
    host.tell_modules_if_untold();
    host.tell_modules_if_untold();
    assert_eq!(
        world.count("api:modules:quality-data@1.2.0/internal/drain"),
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn without_the_api_the_state_is_still_written() {
    let (world, host, root) = host("no-api");
    world.api_down.set(true);
    let mut s = State::new("1.2.0");
    advance(&host, &mut s, Who::Person).unwrap();
    assert_eq!(s.installed_version, "1.3.0");
    assert!(!world.did("api:state"));
    let stored = State::load(&Paths::under(&root).state_file()).unwrap();
    assert_eq!(stored.unwrap().installed_version, "1.3.0");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_restore_that_fails_leaves_every_module_stopped_and_is_reported() {
    let (world, host, root) = host("restore-fails");
    world.unhealthy.set(true);
    world.restore_fails.set(true);
    let mut s = State::new("1.2.0");
    advance(&host, &mut s, Who::Person).unwrap();
    assert!(matches!(s.phase, Phase::Critical { .. }), "{:?}", s.phase);
    let last = s.last.as_ref().unwrap();
    assert_eq!(last.kind, OutcomeKind::Critical);
    assert!(
        last.detail.contains("pg_restore failed on scratch"),
        "{}",
        last.detail
    );
    assert!(
        last.detail.contains("every module is left stopped"),
        "{}",
        last.detail
    );
    let calls = world.calls.borrow().clone();
    let restore = calls.iter().position(|c| c == "database:restore").unwrap();
    // Stopped before the restore and again after it failed; nothing starts.
    assert_eq!(calls[restore - 1], "compose:stop");
    assert_eq!(calls[restore + 1], "compose:stop");
    assert!(
        !calls[restore..].iter().any(|c| c == "compose:start"),
        "{calls:#?}"
    );
    assert!(world.did("cloud:report:Critical"));
    // Critical: the next round does nothing by itself.
    world.calls.borrow_mut().clear();
    advance(&host, &mut s, Who::Service).unwrap();
    assert!(!world.did("compose:start"));
    std::fs::remove_dir_all(root).unwrap();
}
