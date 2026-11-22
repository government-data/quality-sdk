use std::cell::{Cell, RefCell};

use updater::state::Module;

use super::*;

/// The adapters as a fake: what each call did, a call that fails once, and
/// whether the internal certificate exists.
#[derive(Default)]
struct Fake {
    installed: PathBuf,
    calls: RefCell<Vec<String>>,
    fail_once: RefCell<Option<&'static str>>,
    /// A package whose SHA-256 does not match.
    damaged: Option<&'static str>,
    rejected: bool,
    certificate: Cell<bool>,
    /// A `updater/v1` call quality-data does not answer, and how many
    /// times before it answers.
    no_answer: Cell<Option<(&'static str, u32)>>,
    /// quality-data answers another version.
    wrong_version: bool,
}

impl Fake {
    fn call(&self, what: &'static str, detail: &str) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("{what} {detail}"));
        if *self.fail_once.borrow() == Some(what) {
            *self.fail_once.borrow_mut() = None;
            return Err(format!("{what} failed"));
        }
        Ok(())
    }

    fn calls(&self) -> Vec<String> {
        self.calls.take()
    }

    /// An `updater/v1` call: not answered while `no_answer` says so.
    fn api_call(&self, what: &'static str, detail: &str) -> Result<(), CallError> {
        self.call(what, detail).map_err(CallError::Failed)?;
        match self.no_answer.get() {
            Some((w, n)) if w == what && n > 0 => {
                self.no_answer.set(Some((w, n - 1)));
                Err(CallError::NoAnswer(format!(
                    "{what}: quality-data does not answer: invalid peer certificate"
                )))
            }
            _ => Ok(()),
        }
    }
}

fn package(module: &str, requires: &[&str], provides: &[&str]) -> OfferedPackage {
    let names = |l: &[&str]| l.iter().map(ToString::to_string).collect();
    OfferedPackage {
        module: module.into(),
        key: format!("packages/{module}.tar.gz"),
        sha256: "0".repeat(64),
        provides: names(provides),
        requires: names(requires),
    }
}

impl Installer for Fake {
    fn choose(&self, version: Option<&str>) -> Result<Offer, CloudError> {
        let v = version.unwrap_or("1.0.0");
        self.call("choose", v).map_err(CloudError::Network)?;
        if self.rejected {
            return Err(CloudError::Rejected("the index does not verify".into()));
        }
        let module = Module {
            version: v.into(),
            images: [("api".to_string(), "api@sha256:1".to_string())].into(),
        };
        Ok(Offer {
            version: v.into(),
            modules: [
                ("quality-data".into(), module.clone()),
                ("reports".into(), module),
            ]
            .into(),
            // `reports` requires what `quality-data` provides: put after it.
            packages: vec![
                package("reports", &["db"], &[]),
                package("quality-data", &[], &["db"]),
            ],
            notes: String::new(),
            registry: None,
        })
    }
    fn download_package(&self, _o: &Offer, p: &OfferedPackage) -> Result<PathBuf, String> {
        self.call("download", &p.module)?;
        if self.damaged == Some(p.module.as_str()) {
            return Err(format!("{}: the SHA-256 does not match", p.key));
        }
        Ok(PathBuf::from(format!("/packages/{}.tar.gz", p.module)))
    }
    fn pull_images(&self, o: &Offer) -> Result<(), String> {
        self.call("pull", &o.version)
    }
    fn put_package(&self, module: &str, _p: &Path) -> Result<(), String> {
        self.call("put_package", module)
    }
    fn put_version(&self, v: &str, _m: &Modules, order: &[String]) -> Result<(), String> {
        std::fs::create_dir_all(self.installed.parent().unwrap()).unwrap();
        std::fs::write(&self.installed, v).unwrap();
        self.call("put_version", &format!("{v} {}", order.join(",")))
    }
    fn start(&self) -> Result<(), String> {
        self.call("start", "")
    }
    fn internal_certificate(&self) -> bool {
        self.certificate.get()
    }
    fn healthy(&self, version: &str) -> Result<(), CallError> {
        self.api_call("healthy", version)?;
        if self.wrong_version {
            return Err(CallError::Failed(format!("2.0.0 answers, not {version}")));
        }
        Ok(())
    }
    fn tell_modules(&self) -> Result<(), CallError> {
        self.api_call("tell_modules", "")
    }
    fn wait(&self, d: Duration) {
        let _ = self.call("wait", &d.as_secs().to_string());
    }
    fn report(&self, version: &str, failed: Option<&str>) {
        let kind = if failed.is_some() {
            "report_failed"
        } else {
            "report"
        };
        let _ = self.call(kind, version);
    }
}

fn setup(name: &str) -> (Fake, Files, PathBuf) {
    let dir = std::env::temp_dir().join(format!("agent-install-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = Files {
        installed: dir.join("updater/installed.json"),
        progress: dir.join("updater/install.json"),
    };
    let fake = Fake {
        installed: files.installed.clone(),
        ..Fake::default()
    };
    (fake, files, dir)
}

const PREPARE: [&str; 7] = [
    "choose 1.0.0",
    "download quality-data",
    "download reports",
    "pull 1.0.0",
    "put_package quality-data",
    "put_package reports",
    "put_version 1.0.0 quality-data,reports",
];

/// Run it again past the stop after prepare, as the installer does.
fn past_prepared(fake: &Fake, files: &Files) -> Result<Done, String> {
    match install(fake, files, None) {
        Ok(Done::Prepared { .. }) => install(fake, files, None),
        other => other,
    }
}

#[test]
fn a_fresh_install_stops_after_prepare_then_goes_to_the_end() {
    let (fake, files, dir) = setup("fresh");
    fake.certificate.set(true);
    let done = install(&fake, &files, None).unwrap();
    assert_eq!(
        done,
        Done::Prepared {
            version: "1.0.0".into()
        }
    );
    assert_eq!(fake.calls(), PREPARE);
    assert_eq!(
        Progress::load(&files.progress).unwrap().unwrap().step,
        Step::Prepared
    );
    // The next run starts, without preparing again.
    let done = install(&fake, &files, None).unwrap();
    assert_eq!(
        done,
        Done::Installed {
            version: "1.0.0".into()
        }
    );
    assert_eq!(
        fake.calls(),
        ["start ", "healthy 1.0.0", "tell_modules ", "report 1.0.0"]
    );
    assert!(!files.progress.exists() && files.installed.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn without_the_internal_certificate_it_stops_after_the_start_and_resumes() {
    let (fake, files, dir) = setup("certificate");
    install(&fake, &files, Some("1.0.0")).unwrap();
    let done = install(&fake, &files, None).unwrap();
    assert_eq!(
        done,
        Done::NeedsCertificate {
            version: "1.0.0".into()
        }
    );
    assert_eq!(fake.calls().last().unwrap(), "start ");
    assert_eq!(
        Progress::load(&files.progress).unwrap().unwrap().step,
        Step::Verify
    );
    // Run again before it is issued: it stops in the same place, doing nothing.
    assert!(matches!(
        install(&fake, &files, None),
        Ok(Done::NeedsCertificate { .. })
    ));
    assert!(fake.calls().is_empty());
    fake.certificate.set(true);
    assert!(matches!(
        install(&fake, &files, None),
        Ok(Done::Installed { .. })
    ));
    assert_eq!(
        fake.calls(),
        ["healthy 1.0.0", "tell_modules ", "report 1.0.0"]
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_failure_or_a_crash_at_any_step_says_where_and_a_second_run_resumes_there() {
    let resumed: [(&str, &str, &[&str]); 9] = [
        ("choose", "choose", &PREPARE),
        ("download", "prepare", &PREPARE),
        ("pull", "prepare", &PREPARE),
        ("put_package", "prepare", &PREPARE),
        ("put_version", "prepare", &PREPARE),
        ("start", "start", &["start "]),
        ("healthy", "verify", &["healthy 1.0.0"]),
        ("tell_modules", "verify", &["healthy 1.0.0"]),
        ("report", "", &[]),
    ];
    for (fail, step, again) in resumed {
        let (fake, files, dir) = setup(fail);
        fake.certificate.set(true);
        *fake.fail_once.borrow_mut() = Some(fail);
        match past_prepared(&fake, &files) {
            // A report that cannot be written does not stop the install.
            Ok(done) => assert!(fail == "report" && matches!(done, Done::Installed { .. })),
            Err(err) => assert!(
                err.contains(&format!("stopped at {step}: "))
                    && err.contains(&format!("{fail} failed")),
                "{err}"
            ),
        }
        fake.calls();
        assert!(
            matches!(past_prepared(&fake, &files), Ok(Done::Installed { .. })) || fail == "report"
        );
        let calls = fake.calls();
        // The resumed run begins where the failed one stopped (a crash
        // after a step's work and before its progress is saved, as after
        // `put_version`, redoes that step).
        assert!(
            calls.iter().zip(again).all(|(c, a)| c == a),
            "{fail}: {calls:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn an_installation_installed_before_is_refused() {
    let (fake, files, dir) = setup("refused");
    std::fs::create_dir_all(files.installed.parent().unwrap()).unwrap();
    std::fs::write(&files.installed, "{}").unwrap();
    let err = install(&fake, &files, None).unwrap_err();
    assert!(err.contains("installed before"), "{err}");
    assert!(fake.calls().is_empty() && !files.progress.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_install_in_progress_keeps_its_version() {
    let (fake, files, dir) = setup("version");
    install(&fake, &files, Some("1.0.0")).unwrap();
    let err = install(&fake, &files, Some("2.0.0")).unwrap_err();
    assert!(err.contains("an install of 1.0.0 is in progress"), "{err}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_index_that_does_not_verify_starts_nothing() {
    let (mut fake, files, dir) = setup("index");
    fake.rejected = true;
    let err = install(&fake, &files, None).unwrap_err();
    assert!(
        err.contains("the cloud refused: the index does not verify"),
        "{err}"
    );
    assert_eq!(fake.calls(), ["choose 1.0.0"]);
    assert!(!files.progress.exists() && !files.installed.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_damaged_package_prepares_nothing() {
    let (mut fake, files, dir) = setup("damaged");
    fake.damaged = Some("reports");
    let err = install(&fake, &files, None).unwrap_err();
    assert!(
        err.contains("stopped at prepare") && err.contains("SHA-256"),
        "{err}"
    );
    let calls = fake.calls();
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("pull") || c.starts_with("put")),
        "{calls:?}"
    );
    // A failure after the version was chosen is reported.
    assert_eq!(calls.last().unwrap(), "report_failed 1.0.0");
    assert!(!files.installed.exists());
    assert_eq!(
        Progress::load(&files.progress).unwrap().unwrap().step,
        Step::Prepare
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// Installed, up to the verify step with the internal certificate issued.
fn at_verify(name: &str) -> (Fake, Files, PathBuf) {
    let (fake, files, dir) = setup(name);
    install(&fake, &files, None).unwrap();
    assert!(matches!(
        install(&fake, &files, None),
        Ok(Done::NeedsCertificate { .. })
    ));
    fake.certificate.set(true);
    fake.calls();
    (fake, files, dir)
}

#[test]
fn a_call_not_answered_yet_is_tried_again_until_it_answers() {
    // As Traefik serving another certificate until it loads the internal
    // entry point's files, just written by the installer.
    for what in ["healthy", "tell_modules"] {
        let (fake, files, dir) = at_verify(&format!("retry-{what}"));
        fake.no_answer.set(Some((what, 3)));
        assert!(matches!(
            install(&fake, &files, None),
            Ok(Done::Installed { .. })
        ));
        let calls = fake.calls();
        let waits = calls.iter().filter(|c| *c == "wait 5").count();
        let tries = calls.iter().filter(|c| c.starts_with(what)).count();
        assert_eq!((waits, tries), (3, 4), "{calls:?}");
        assert_eq!(calls.last().unwrap(), "report 1.0.0");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn a_call_never_answered_fails_after_the_window_and_resumes() {
    let (fake, files, dir) = at_verify("never");
    fake.no_answer.set(Some(("healthy", u32::MAX)));
    let err = install(&fake, &files, None).unwrap_err();
    assert!(
        err.contains("stopped at verify") && err.contains("does not answer"),
        "{err}"
    );
    let calls = fake.calls();
    let waits = calls.iter().filter(|c| *c == "wait 5").count();
    let every = ANSWER_EVERY.as_secs();
    assert_eq!(waits as u64 * every, ANSWER_WITHIN.as_secs(), "{calls:?}");
    assert!(!calls.iter().any(|c| c.starts_with("tell_modules")));
    assert_eq!(calls.last().unwrap(), "report_failed 1.0.0");
    assert_eq!(
        Progress::load(&files.progress).unwrap().unwrap().step,
        Step::Verify
    );
    fake.no_answer.set(None);
    assert!(matches!(
        install(&fake, &files, None),
        Ok(Done::Installed { .. })
    ));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_wrong_answer_fails_at_once() {
    // Another version answering is not tried again.
    let (mut fake, files, dir) = at_verify("wrong");
    fake.wrong_version = true;
    let err = install(&fake, &files, None).unwrap_err();
    assert!(err.contains("2.0.0 answers, not 1.0.0"), "{err}");
    assert_eq!(fake.calls(), ["healthy 1.0.0", "report_failed 1.0.0"]);
    std::fs::remove_dir_all(dir).unwrap();
    // Nor an HTTP error answer (or any failure that is not "no answer").
    let (fake, files, dir) = at_verify("http");
    *fake.fail_once.borrow_mut() = Some("tell_modules");
    let err = install(&fake, &files, None).unwrap_err();
    assert!(err.contains("tell_modules failed"), "{err}");
    assert!(!fake.calls().iter().any(|c| c.starts_with("wait")));
    std::fs::remove_dir_all(dir).unwrap();
}
