use std::cell::RefCell;

use chrono::TimeZone;

use super::*;

const PASSWORD: &str = "the-token-password";

struct FakeEcr {
    fails: Cell<bool>,
    asked: Cell<u32>,
}

impl TokenSource for FakeEcr {
    fn token(&self, now: DateTime<Utc>) -> Result<RegistryToken, String> {
        self.asked.set(self.asked.get() + 1);
        if self.fails.get() {
            return Err("ECR GetAuthorizationToken: unavailable".into());
        }
        Ok(RegistryToken {
            host: "1.dkr.ecr.us-east-1.amazonaws.com".into(),
            username: "AWS".into(),
            password: PASSWORD.into(),
            expires_at: now + LIFE,
        })
    }
}

#[derive(Default)]
struct FakeCore {
    features: Vec<&'static str>,
    refuses: Cell<bool>,
    handed: RefCell<Vec<RegistryToken>>,
}

impl TokenSink for FakeCore {
    fn features(&self) -> Result<Features, String> {
        Ok(Features(
            self.features.iter().map(|f| f.to_string()).collect(),
        ))
    }
    fn put_registry(&self, token: &RegistryToken) -> Result<(), String> {
        if self.refuses.get() {
            return Err("PUT /updater/v1/registry: quality-data answered 503".into());
        }
        self.handed.borrow_mut().push(token.clone());
        Ok(())
    }
}

fn ecr() -> FakeEcr {
    FakeEcr {
        fails: Cell::new(false),
        asked: Cell::new(0),
    }
}

fn core() -> FakeCore {
    FakeCore {
        features: vec![Features::REGISTRY_CREDENTIALS],
        ..FakeCore::default()
    }
}

fn at(h: i64, m: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 25, 0, 0, 0).unwrap() + Duration::hours(h) + Duration::minutes(m)
}

#[test]
fn handed_at_start_then_every_six_hours() {
    let (supply, ecr, core) = (Supply::default(), ecr(), core());
    assert_eq!(
        supply.supply_if_due(at(0, 0), &ecr, &core),
        Supplied::Handed(at(12, 0))
    );
    assert_eq!(
        supply.supply_if_due(at(0, 1), &ecr, &core),
        Supplied::NotDue
    );
    assert_eq!(
        supply.supply_if_due(at(5, 59), &ecr, &core),
        Supplied::NotDue
    );
    assert!(matches!(
        supply.supply_if_due(at(6, 0), &ecr, &core),
        Supplied::Handed(_)
    ));
    assert_eq!(ecr.asked.get(), 2);
    let handed = core.handed.borrow();
    assert_eq!(handed.len(), 2);
    assert_eq!(handed[0].username, "AWS");
    assert_eq!(handed[0].password, PASSWORD);
}

#[test]
fn handed_again_once_the_modules_were_told_anew() {
    let (supply, ecr, core) = (Supply::default(), ecr(), core());
    supply.supply_if_due(at(0, 0), &ecr, &core);
    supply.again_now();
    assert!(matches!(
        supply.supply_if_due(at(0, 30), &ecr, &core),
        Supplied::Handed(_)
    ));
    assert_eq!(core.handed.borrow().len(), 2);
}

#[test]
fn a_core_without_the_feature_gets_nothing_and_is_asked_again_later() {
    let (supply, ecr) = (Supply::default(), ecr());
    let core = FakeCore::default();
    assert_eq!(
        supply.supply_if_due(at(0, 0), &ecr, &core),
        Supplied::NotOffered
    );
    assert_eq!(
        ecr.asked.get(),
        0,
        "ECR is not asked for a core that cannot use it"
    );
    assert_eq!(
        supply.supply_if_due(at(0, 9), &ecr, &core),
        Supplied::NotDue
    );
    assert_eq!(
        supply.supply_if_due(at(0, 10), &ecr, &core),
        Supplied::NotOffered
    );
}

#[test]
fn a_failure_is_tried_again_ten_minutes_later() {
    let (supply, ecr, core) = (Supply::default(), ecr(), core());
    ecr.fails.set(true);
    assert!(matches!(
        supply.supply_if_due(at(0, 0), &ecr, &core),
        Supplied::Failed(e) if e.contains("ECR")
    ));
    assert_eq!(
        supply.supply_if_due(at(0, 5), &ecr, &core),
        Supplied::NotDue
    );
    ecr.fails.set(false);
    core.refuses.set(true);
    let failed = supply.supply_if_due(at(0, 10), &ecr, &core);
    assert!(matches!(&failed, Supplied::Failed(e) if e.contains("503")));
    assert!(!format!("{failed:?}").contains(PASSWORD));
    core.refuses.set(false);
    assert!(matches!(
        supply.supply_if_due(at(0, 20), &ecr, &core),
        Supplied::Handed(_)
    ));
}

#[test]
fn the_password_is_never_printed() {
    let token = ecr().token(at(0, 0)).unwrap();
    let printed = format!("{token:?}");
    assert!(printed.contains("1.dkr.ecr.us-east-1.amazonaws.com"));
    assert!(!printed.contains(PASSWORD));
}
