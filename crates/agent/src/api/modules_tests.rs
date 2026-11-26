//! A module's clients, purge and audit against the fake server of
//! `api/tests.rs`. Every secret here is invented.

use serde_json::{json, Value};

use super::*;
use crate::api::no_answer;
use crate::api::tests::{api, closed, server};

const SECRET: &str = "qs-test-people-secret-not-real";
const MACHINE: &str = "qs-test-machine-secret-not-real";

fn clients() -> Clients {
    Clients {
        people: Some(PeopleClient {
            redirect_paths: vec!["/bi/oauth-authorized/qd".into()],
            logout_paths: vec!["/bi/".into()],
            alg: TokenAlg::ES256,
            pkce: true,
            role_claim: Some(RoleClaim {
                name: "bi_roles".into(),
                values: [("author".to_string(), "bi:Author".to_string())].into(),
            }),
            secret: Secret::new(SECRET.into()),
            keep_previous: false,
        }),
        machine: Some(MachineClient {
            capabilities: vec!["sql-query".into()],
            audiences: vec!["trino".into()],
            requires: Vec::new(),
            secret: Secret::new(MACHINE.into()),
            keep_previous: true,
        }),
    }
}

fn event(name: &str) -> AuditEvent {
    AuditEvent {
        event: name.into(),
        module: "bi".into(),
        version: "1.0.0".into(),
        outcome: "succeeded".into(),
        step: None,
        error: None,
    }
}

#[test]
fn the_clients_are_put_with_their_secrets_and_the_answer_read() {
    let answer = r#"{"issuer":"https://qd.example/auth","people_client_id":"bi",
        "machine_client_id":"bi-machine"}"#;
    let (base, sent) = server(&[("PUT /updater/v1/modules/bi/clients", 200, answer)]);
    let got = api(&base).put_clients("bi", &clients()).unwrap();
    assert_eq!(got.issuer, "https://qd.example/auth");
    assert_eq!(got.machine_client_id.as_deref(), Some("bi-machine"));
    let body: Value = serde_json::from_str(&sent.lock().unwrap()[0].2).unwrap();
    assert_eq!(
        body,
        json!({
            "people": {"redirect_paths": ["/bi/oauth-authorized/qd"], "logout_paths": ["/bi/"],
                       "alg": "ES256", "pkce": true,
                       "role_claim": {"name": "bi_roles", "values": {"author": "bi:Author"}},
                       "secret": SECRET, "keep_previous": false},
            "machine": {"capabilities": ["sql-query"], "audiences": ["trino"],
                        "secret": MACHINE, "keep_previous": true},
        })
    );
    // A client the module does not have is not named.
    let (base, sent) = server(&[("PUT /updater/v1/modules/bi/clients", 200, answer)]);
    let only = Clients {
        machine: None,
        ..clients()
    };
    api(&base).put_clients("bi", &only).unwrap();
    let body: Value = serde_json::from_str(&sent.lock().unwrap()[0].2).unwrap();
    assert!(body.get("machine").is_none(), "{body}");
}

#[test]
fn the_machine_client_names_the_requires_when_there_are_some() {
    let mut c = clients();
    c.machine.as_mut().unwrap().requires = vec!["orchestration".into()];
    let body = serde_json::to_value(&c).unwrap();
    assert_eq!(body["machine"]["requires"], json!(["orchestration"]));
}

#[test]
fn no_secret_reaches_an_error_or_a_debug_print() {
    let (base, _) = server(&[(
        "PUT /updater/v1/modules/bi/clients",
        409,
        r#"{"error":"the client bi was registered by hand"}"#,
    )]);
    let err = api(&base).put_clients("bi", &clients()).unwrap_err();
    assert!(
        err.contains("409 the client bi was registered by hand"),
        "{err}"
    );
    let printed = format!("{:?} {err}", clients());
    assert!(
        !printed.contains(SECRET) && !printed.contains(MACHINE),
        "{printed}"
    );
    let err = api(&closed()).put_clients("bi", &clients()).unwrap_err();
    assert!(no_answer(&err) && !err.contains(SECRET), "{err}");
    assert!(api(&base).put_clients("../x", &clients()).is_err());
}

#[test]
fn clients_are_disabled_and_a_module_deleted() {
    let (base, sent) = server(&[
        ("POST /updater/v1/modules/bi/clients/disable", 204, ""),
        ("DELETE /updater/v1/modules/bi", 204, ""),
    ]);
    api(&base).disable_clients("bi").unwrap();
    api(&base).delete_module("bi").unwrap();
    let sent = sent.lock().unwrap();
    let calls: Vec<(&str, &str)> = sent
        .iter()
        .map(|(m, p, _)| (m.as_str(), p.as_str()))
        .collect();
    assert_eq!(
        calls,
        [
            ("POST", "/updater/v1/modules/bi/clients/disable"),
            ("DELETE", "/updater/v1/modules/bi"),
        ]
    );
    assert!(no_answer(&api(&closed()).delete_module("bi").unwrap_err()));
}

#[test]
fn an_event_the_core_does_not_take_waits_and_is_sent_in_order() {
    let dir = std::env::temp_dir().join(format!(
        "qs-audit-outbox-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let outbox = Outbox {
        path: dir.join("updater/audit-outbox.jsonl"),
    };
    let down = api(&closed());
    outbox.send(&down, &event("ModuleAdded")).unwrap();
    let mut failed = event("ModuleRemoved");
    failed.outcome = "failed".into();
    failed.step = Some("down".into());
    failed.error = Some("docker compose down failed".into());
    outbox.send(&down, &failed).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&outbox.path)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(outbox.flush(&down).unwrap(), 0);
    let (base, sent) = server(&[("POST /updater/v1/audit", 204, "")]);
    assert_eq!(outbox.flush(&api(&base)).unwrap(), 2);
    assert!(!outbox.path.exists());
    let sent = sent.lock().unwrap();
    let first: Value = serde_json::from_str(&sent[0].2).unwrap();
    let second: Value = serde_json::from_str(&sent[1].2).unwrap();
    assert_eq!(first["event"], "ModuleAdded");
    assert!(first.get("step").is_none(), "{first}");
    assert_eq!(
        second,
        json!({"event": "ModuleRemoved", "module": "bi", "version": "1.0.0",
               "outcome": "failed", "step": "down", "error": "docker compose down failed"})
    );
    // Taken at once: nothing waits.
    let (base, _) = server(&[("POST /updater/v1/audit", 204, "")]);
    outbox.send(&api(&base), &event("ModulePurged")).unwrap();
    assert!(!outbox.path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
