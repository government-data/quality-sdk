//! The `updater/v1` client against a local fake server: an HTTP/1.1
//! listener in this process that answers each call from a table and keeps
//! what it was sent.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aws_client::http::Https;
use updater::policy::Mode;
use updater::state::State;

use super::*;

/// What the fake server was sent: method, path and body.
pub(super) type Sent = Arc<Mutex<Vec<(String, String, String)>>>;

/// A server that answers `answers` (by method and path) and 404 otherwise.
pub(super) fn server(answers: &[(&str, u16, &str)]) -> (String, Sent) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let answers: Vec<(String, u16, String)> = answers
        .iter()
        .map(|(w, s, b)| ((*w).to_string(), *s, (*b).to_string()))
        .collect();
    let sent: Sent = Arc::default();
    let log = sent.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut parts = line.split_whitespace();
            let (method, path) = (
                parts.next().unwrap_or("").to_string(),
                parts.next().unwrap_or("").to_string(),
            );
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
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let what = format!("{method} {path}");
            let (status, answer) = answers
                .iter()
                .find(|(w, _, _)| *w == what)
                .map_or((404, String::new()), |(_, s, b)| (*s, b.clone()));
            log.lock()
                .unwrap()
                .push((method, path, String::from_utf8(body).unwrap()));
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    (base, sent)
}

pub(super) fn api(base: &str) -> UpdaterApi {
    UpdaterApi::new(base, Box::new(Https::default()))
}

#[test]
fn the_state_is_put_as_the_state_file_s_document() {
    let (base, sent) = server(&[("PUT /updater/v1/state", 204, "")]);
    let state = State::new("1.3.0");
    api(&base).put_state(&state).unwrap();
    let sent = sent.lock().unwrap();
    assert_eq!(sent[0].1, "/updater/v1/state");
    let body: serde_json::Value = serde_json::from_str(&sent[0].2).unwrap();
    assert_eq!(body, serde_json::to_value(&state).unwrap());
}

#[test]
fn orders_are_read_and_without_an_answer_it_is_approval_and_no_request() {
    let (base, _) = server(&[(
        "GET /updater/v1/orders",
        200,
        r#"{"policy":{"mode":"automatic","paused":false},"request":{"id":"r-1"}}"#,
    )]);
    let o = api(&base).orders();
    assert_eq!(o.policy.mode, Mode::Automatic);
    assert_eq!(o.request.unwrap().id, "r-1");

    let (base, _) = server(&[("GET /updater/v1/orders", 500, r#"{"error":"busy"}"#)]);
    assert_eq!(api(&base).orders(), Orders::default());
    let (base, _) = server(&[("GET /updater/v1/orders", 200, "not json")]);
    assert_eq!(api(&base).orders(), Orders::default());
    assert_eq!(api(&closed()).orders(), Orders::default());
}

#[test]
fn maintenance_is_entered_for_a_time_and_left() {
    let (base, sent) = server(&[("PUT /updater/v1/maintenance", 200, "{}")]);
    let a = api(&base);
    a.maintenance(Some("updating to 1.4.0"), Duration::from_secs(7200))
        .unwrap();
    a.maintenance(None, Duration::ZERO).unwrap();
    let sent = sent.lock().unwrap();
    let body = |i: usize| serde_json::from_str::<serde_json::Value>(&sent[i].2).unwrap();
    assert_eq!(
        body(0),
        json!({"active": true, "reason": "updating to 1.4.0", "secs": 7200})
    );
    assert_eq!(body(1), json!({"active": false}));
}

#[test]
fn the_drain_is_asked_with_its_horizon() {
    let (base, sent) = server(&[(
        "GET /updater/v1/drain?horizon_secs=900",
        200,
        r#"{"drained":false,"running":2,"deploying":1,"scheduled_soon":3}"#,
    )]);
    let d = api(&base).drain(Duration::from_secs(900)).unwrap();
    assert_eq!(
        d,
        Drain {
            drained: false,
            running: 2,
            deploying: 1,
            scheduled_soon: 3
        }
    );
    assert_eq!(
        sent.lock().unwrap()[0].1,
        "/updater/v1/drain?horizon_secs=900"
    );
    let (base, _) = server(&[("GET /updater/v1/drain?horizon_secs=1", 200, "{}")]);
    assert!(api(&base)
        .drain(Duration::from_secs(1))
        .unwrap_err()
        .contains("drained"));
}

#[test]
fn the_version_that_answers_is_read_and_a_refusal_says_why() {
    let (base, _) = server(&[("GET /updater/v1/version", 200, r#"{"version":"1.4.0"}"#)]);
    assert_eq!(api(&base).version().unwrap(), "1.4.0");
    let (base, _) = server(&[("GET /updater/v1/version", 503, r#"{"error":"starting"}"#)]);
    let err = api(&base).version().unwrap_err();
    assert!(err.contains("503 starting"), "{err}");
    let (base, _) = server(&[("GET /updater/v1/version", 200, r#"{"version":""}"#)]);
    assert!(api(&base).version().is_err());
}

#[test]
fn the_installed_modules_are_put_whole() {
    let (base, sent) = server(&[("PUT /updater/v1/modules", 204, "")]);
    let manifest = |text: &str| module_manifest::Manifest::parse(text).unwrap();
    let orchestrator =
        manifest("name: orchestrator\nversion: 2.0.1\ndrain: { path: /internal/drain }\n");
    let docs = manifest(
        "name: docs\nversion: 0.2.0\nroute: { path: /docs, service: web, port: 8080 }\n\
         auth: forward\nmenu: { label: { en: User guide, es: Guía de usuario }, \
         path: { en: /docs/, es: /docs/es/ } }\n",
    );
    let none = Features::default();
    let modules = [
        InstalledModule::new("orchestrator", "2.0.1", Some(&orchestrator), &none),
        InstalledModule::new("quality-data", "1.4.0", None, &none),
        InstalledModule::new("docs", "0.2.0", Some(&docs), &none),
    ];
    api(&base).put_modules(&modules).unwrap();
    let sent = sent.lock().unwrap();
    assert_eq!(
        (sent[0].0.as_str(), sent[0].1.as_str()),
        ("PUT", "/updater/v1/modules")
    );
    let body: Value = serde_json::from_str(&sent[0].2).unwrap();
    assert_eq!(
        body,
        json!({"modules": [
            {"name": "orchestrator", "version": "2.0.1", "drain_path": "/internal/drain",
             "route_path": null, "menu": null},
            {"name": "quality-data", "version": "1.4.0", "drain_path": null,
             "route_path": null, "menu": null},
            {"name": "docs", "version": "0.2.0", "drain_path": null, "route_path": "/docs",
             "menu": {"label": {"en": "User guide", "es": "Guía de usuario"},
                      "path": {"en": "/docs/", "es": "/docs/es/"}}},
        ]})
    );
    let (base, _) = server(&[("PUT /updater/v1/modules", 400, r#"{"error":"bad module"}"#)]);
    let err = api(&base).put_modules(&[]).unwrap_err();
    assert!(err.contains("400 bad module"), "{err}");
    // An HTTP error is an answer.
    assert!(!no_answer(&err), "{err}");
}

/// A module with a menu condition and permissions (invented).
const BI: &str = "name: bi\nversion: 1.0.0\nroute: { path: /bi, service: web, port: 8080 }\n\
    menu: { label: { en: BI, es: BI }, requires: 'bi:Author' }\n\
    permissions:\n  actions: [{ name: 'bi:Author', description: { en: Author, es: Autor } }]\n";

#[test]
fn a_menu_condition_and_permissions_go_to_a_core_that_has_their_features() {
    let bi = module_manifest::Manifest::parse(BI).unwrap();
    // An older core: no menu (hidden from everyone), no permissions.
    let older = InstalledModule::new("bi", "1.0.0", Some(&bi), &Features::default());
    assert_eq!(
        (older.menu.as_ref(), older.permissions.as_ref()),
        (None, None)
    );
    let body = serde_json::to_value(&older).unwrap();
    assert!(body.get("permissions").is_none(), "{body}");
    let features = Features(
        ["menu-requires", "module-permissions"]
            .map(String::from)
            .into(),
    );
    let newer = InstalledModule::new("bi", "1.0.0", Some(&bi), &features);
    let body = serde_json::to_value(&newer).unwrap();
    assert_eq!(body["menu"]["requires"], "bi:Author");
    assert_eq!(body["permissions"]["actions"][0]["name"], "bi:Author");
    assert_eq!(
        body["permissions"]["actions"][0]["description"]["es"],
        "Autor"
    );
    assert_eq!(body["permissions"]["policies"], json!([]));
    // A menu without a condition is sent to any core, without `requires`.
    let plain = BI.replace(", requires: 'bi:Author'", "");
    let plain = module_manifest::Manifest::parse(&plain).unwrap();
    let m = InstalledModule::new("bi", "1.0.0", Some(&plain), &Features::default());
    let body = serde_json::to_value(&m).unwrap();
    assert!(body["menu"].get("requires").is_none(), "{body}");
}

#[test]
fn the_features_are_read_and_a_core_without_them_has_none() {
    let (base, _) = server(&[(
        "GET /updater/v1/version",
        200,
        r#"{"version":"1.5.0","features":["menu-requires","module-clients"]}"#,
    )]);
    let f = api(&base).features().unwrap();
    assert!(f.has(Features::MENU_REQUIRES) && f.has(Features::MODULE_CLIENTS));
    assert!(!f.has(Features::AGENT_AUDIT));
    let (base, _) = server(&[("GET /updater/v1/version", 200, r#"{"version":"1.4.0"}"#)]);
    assert_eq!(api(&base).features().unwrap(), Features::default());
    assert!(no_answer(&api(&closed()).features().unwrap_err()));
}

#[test]
fn a_module_s_replica_is_asked_and_a_core_without_the_route_is_an_error() {
    let (base, sent) = server(&[(
        "GET /updater/v1/modules/lake-rustfs/replica",
        200,
        r#"{"state":"behind"}"#,
    )]);
    assert_eq!(
        api(&base).replica("lake-rustfs").unwrap(),
        ReplicaState::Behind
    );
    assert_eq!(
        sent.lock().unwrap()[0].1,
        "/updater/v1/modules/lake-rustfs/replica"
    );
    let (base, _) = server(&[(
        "GET /updater/v1/modules/lake-rustfs/replica",
        200,
        r#"{"state":"resyncing"}"#,
    )]);
    assert_eq!(
        api(&base).replica("lake-rustfs").unwrap(),
        ReplicaState::Unknown
    );
    let (base, _) = server(&[("GET /updater/v1/modules/lake-rustfs/replica", 200, "{}")]);
    let err = api(&base).replica("lake-rustfs").unwrap_err();
    assert!(err.contains("no `state`"), "{err}");
    // An older core has no such route.
    let (base, _) = server(&[]);
    assert!(api(&base).replica("lake-rustfs").is_err());
}

#[test]
fn a_module_says_the_capabilities_it_provides_and_none_is_not_sent() {
    let lake = module_manifest::Manifest::parse(
        "name: lake-rustfs\nversion: 0.1.0\nprovides:\n  capabilities:\n    \
         - { name: lake, version: 1, protocols: [s3] }\n",
    )
    .unwrap();
    let m = InstalledModule::new("lake-rustfs", "0.1.0", Some(&lake), &Features::default());
    let body = serde_json::to_value(&m).unwrap();
    assert_eq!(body["provides"], json!(["lake"]));
    let docs = InstalledModule::new("docs", "0.2.0", None, &Features::default());
    let body = serde_json::to_value(&docs).unwrap();
    assert!(body.get("provides").is_none(), "{body}");
}

#[test]
fn an_api_that_does_not_answer_fails_every_call_but_orders() {
    let a = api(&closed());
    let err = a.version().unwrap_err();
    assert!(err.contains("does not answer") && no_answer(&err), "{err}");
    assert!(no_answer(&a.put_modules(&[]).unwrap_err()));
    assert!(a.put_state(&State::new("1.0.0")).is_err());
    assert!(a.maintenance(None, Duration::ZERO).is_err());
    assert!(a.drain(Duration::ZERO).is_err());
    assert!(a.put_modules(&[]).is_err());
}

/// An address where nothing listens.
pub(super) fn closed() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    base
}

#[test]
fn an_update_is_requested_and_a_refusal_says_why() {
    let filed = r#"{"id":"r-9","at":"2026-11-22T10:00:00Z","by":"updater"}"#;
    let (base, sent) = server(&[("POST /updater/v1/request", 202, filed)]);
    assert_eq!(api(&base).request_update().unwrap(), "r-9");
    let sent = sent.lock().unwrap();
    assert_eq!((sent[0].0.as_str(), sent[0].2.as_str()), ("POST", "{}"));

    let busy = r#"{"error":"an update is in progress"}"#;
    let (base, _) = server(&[("POST /updater/v1/request", 409, busy)]);
    let err = api(&base).request_update().unwrap_err();
    assert!(err.contains("409 an update is in progress"), "{err}");
    let (base, _) = server(&[]);
    let err = api(&base).request_update().unwrap_err();
    assert!(err.contains("answered 404"), "an older quality-data: {err}");
    assert!(no_answer(&api(&closed()).request_update().unwrap_err()));
}
