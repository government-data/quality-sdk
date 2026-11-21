//! Each case of the update diagram, with a fake host that writes down what it
//! is asked and fails where told.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use chrono::TimeZone;

use super::*;
use crate::policy::{Mode, Policy, Request, Window};

mod preparing;
mod recovering;
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
    /// What fails: "package", "backup", "start:1.3.0", "healthy:1.3.0",
    /// "healthy:previous", "restore"…
    fails: RefCell<HashSet<&'static str>>,
    /// The version brought up.
    up: RefCell<String>,
    calls: RefCell<Vec<String>>,
    saved: RefCell<Vec<State>>,
    maintenance: Cell<bool>,
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

fn offer(version: &str) -> Offer {
    Offer {
        version: version.into(),
        api: format!("r/api:{version}@sha256:a"),
        web: format!("r/web:{version}@sha256:b"),
        package_url: format!("https://s3/quality-data-{version}.tar.gz"),
        package_sha256: "abc".into(),
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
    fn download_package(&self, o: &Offer) -> Result<PathBuf, String> {
        self.note(format!("download_package:{}", o.version));
        if self.fails_at("package") {
            return Err("the SHA-256 does not match".into());
        }
        Ok(format!("/packages/{}.tar.gz", o.version).into())
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
            api: "r/api:1.2.0".into(),
            web: "r/web:1.2.0".into(),
        })
    }
    fn back_up(&self) -> Result<PathBuf, String> {
        self.note("back_up");
        if self.fails_at("backup") {
            return Err("pg_restore --list failed".into());
        }
        Ok("/backups/quality-data-1".into())
    }
    fn save_package(&self, version: &str) -> Result<(PathBuf, Vec<String>), String> {
        self.note(format!("save_package:{version}"));
        Ok((
            format!("/releases/{version}.tar").into(),
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
    fn put_package(&self, package: &Path, remove: &[String]) -> Result<(), String> {
        self.note(format!(
            "put_package:{}:remove={}",
            package.display(),
            remove.join(",")
        ));
        Ok(())
    }
    fn put_version(&self, version: &str, _api: &str, _web: &str) -> Result<(), String> {
        self.note(format!("put_version:{version}"));
        *self.up.borrow_mut() = version.into();
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
