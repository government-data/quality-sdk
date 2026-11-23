//! The agent's loop: rounds of the engine through the host's composition,
//! with fake adapters for the cloud, Compose, the database and the files,
//! and the real `updater/v1` client against a fake server of the protocol.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent::api::UpdaterApi;
use agent::host::{Adapters, AgentHost, Clock, Cloud, Compose, Database, Packages};
use agent::service::{round, run, run_with};
use aws_client::http::Https;
use chrono::{DateTime, TimeZone, Utc};
use module_manifest::Manifest;
use updater::engine::{CloudError, Installed, Offer, OfferedPackage};
use updater::state::{Modules, Outcome, State};

/// What the fake `updater/v1` server was asked: method and path.
type Asked = Arc<Mutex<Vec<String>>>;

/// A server of `updater/v1` that answers every call with `{}` but
/// `GET /updater/v1/orders`, approval and no request.
fn updater_v1() -> (String, Asked) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let asked: Asked = Arc::default();
    let log = asked.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let what: Vec<&str> = line.split_whitespace().take(2).collect();
            let what = what.join(" ");
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = header.split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        length = v.trim().parse().unwrap();
                    }
                }
            }
            reader.read_exact(&mut vec![0; length]).unwrap();
            let answer = if what == "GET /updater/v1/orders" {
                r#"{"policy":{"mode":"approval","paused":false},"request":null}"#
            } else {
                "{}"
            };
            log.lock().unwrap().push(what);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    (base, asked)
}

/// What the fakes share.
#[derive(Default)]
struct World {
    checked: Cell<u32>,
    /// What `installed` answers; `None` is an error.
    installed: RefCell<Option<String>>,
    waited: RefCell<Vec<Duration>>,
}

#[derive(Clone)]
struct Fakes(std::rc::Rc<World>);

impl Cloud for Fakes {
    /// Nothing newer than what is installed.
    fn check(&self, installed: &str) -> Result<Offer, CloudError> {
        self.0.checked.set(self.0.checked.get() + 1);
        Ok(Offer {
            version: installed.into(),
            modules: Modules::new(),
            packages: Vec::new(),
            notes: String::new(),
            registry: None,
        })
    }
    fn report(&self, _o: &Outcome) {}
    fn download_package(&self, _o: &Offer, _p: &OfferedPackage) -> Result<PathBuf, String> {
        Err("not offered".into())
    }
    fn pull_images(&self, _o: &Offer) -> Result<(), String> {
        Err("not offered".into())
    }
}

impl Compose for Fakes {
    fn put_version(&self, _v: &str, _m: &Modules, _o: &[String]) -> Result<(), String> {
        Err("not offered".into())
    }
    fn start(&self) -> Result<(), String> {
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        Ok(())
    }
    fn installed(&self) -> Result<Installed, String> {
        let version = self.0.installed.borrow().clone();
        version
            .map(|version| Installed {
                version,
                modules: Modules::new(),
            })
            .ok_or_else(|| "no platform version is recorded".to_string())
    }
    fn modules_healthy(&self) -> Result<(), String> {
        Ok(())
    }
    fn manifest(&self, _module: &str) -> Result<Option<Manifest>, String> {
        Ok(None)
    }
}

impl Database for Fakes {
    fn back_up(&self) -> Result<PathBuf, String> {
        Err("not offered".into())
    }
    fn restore(&self, _b: &Path) -> Result<(), String> {
        Err("not offered".into())
    }
}

impl Packages for Fakes {
    fn free_space(&self) -> Result<(), String> {
        Ok(())
    }
    fn save(&self, _m: &str, _v: &str) -> Result<(PathBuf, Vec<String>), String> {
        Err("not offered".into())
    }
    fn manifest(&self, _p: &Path) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    fn put(&self, _m: &str, _p: &Path, _r: &[String]) -> Result<(), String> {
        Err("not offered".into())
    }
    fn install_commands(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Clock for Fakes {
    fn now(&self) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 21, 12, 0, 0).unwrap()
    }
    fn wait(&self, d: Duration) {
        self.0.waited.borrow_mut().push(d);
    }
}

fn host(state_file: &Path, installed: Option<&str>) -> (AgentHost, Fakes, Asked) {
    let (base, asked) = updater_v1();
    let fakes = Fakes(std::rc::Rc::default());
    *fakes.0.installed.borrow_mut() = installed.map(str::to_string);
    let adapters = Adapters {
        cloud: Box::new(fakes.clone()),
        compose: Box::new(fakes.clone()),
        database: Box::new(fakes.clone()),
        packages: Box::new(fakes.clone()),
        api: Box::new(UpdaterApi::new(&base, Box::new(Https::default()))),
        clock: Box::new(fakes.clone()),
    };
    (AgentHost::new(adapters, state_file.into()), fakes, asked)
}

fn state_file(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-service-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d.join("var/lib/quality-data/updater/state.json")
}

#[test]
fn a_first_round_starts_a_state_at_the_installed_version_and_asks_the_cloud() {
    let file = state_file("first");
    let (host, fakes, asked) = host(&file, Some("1.2.0"));
    round(&host, &file).unwrap();
    let state = State::load(&file).unwrap().unwrap();
    assert_eq!(state.installed_version, "1.2.0");
    assert!(state.last_check.is_some());
    assert_eq!(fakes.0.checked.get(), 1);
    let asked = asked.lock().unwrap();
    assert_eq!(asked[0], "PUT /updater/v1/state");
    assert!(asked.contains(&"GET /updater/v1/orders".to_string()));
    std::fs::remove_dir_all(file.ancestors().nth(4).unwrap()).unwrap();
}

#[test]
fn rounds_wait_between_them_and_the_cloud_is_asked_once_an_hour() {
    let file = state_file("rounds");
    let (host, fakes, _) = host(&file, Some("1.2.0"));
    run(&host, &file, Duration::from_secs(60), Some(3));
    assert_eq!(*fakes.0.waited.borrow(), [Duration::from_secs(60); 2]);
    // The clock stands still: the state file's last check is not an hour
    // old, so only the first round asked.
    assert_eq!(fakes.0.checked.get(), 1);
    std::fs::remove_dir_all(file.ancestors().nth(4).unwrap()).unwrap();
}

#[test]
fn a_round_that_fails_is_logged_and_the_next_one_runs() {
    let file = state_file("fails");
    let (host, fakes, _) = host(&file, None);
    let err = round(&host, &file).unwrap_err();
    assert!(err.contains("no platform version"), "{err}");
    run(&host, &file, Duration::from_secs(5), Some(2));
    assert_eq!(fakes.0.waited.borrow().len(), 1);
    assert_eq!(fakes.0.checked.get(), 0);
    assert!(!file.exists());
    *fakes.0.installed.borrow_mut() = Some("1.2.0".into());
    run(&host, &file, Duration::from_secs(5), Some(1));
    assert_eq!(fakes.0.checked.get(), 1);
    std::fs::remove_dir_all(file.ancestors().nth(4).unwrap()).unwrap();
}

#[test]
fn each_round_is_preceded_by_the_chores_before_it() {
    let file = state_file("before");
    let (host, fakes, _) = host(&file, Some("1.2.0"));
    let mut before = Vec::new();
    run_with(&host, &file, Duration::from_secs(60), Some(3), &mut || {
        before.push(fakes.0.waited.borrow().len());
    });
    // Before the first round, and after each wait.
    assert_eq!(before, [0, 1, 2]);
    std::fs::remove_dir_all(file.ancestors().nth(4).unwrap()).unwrap();
}
