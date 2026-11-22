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
type Sent = Arc<Mutex<Vec<(String, String, String)>>>;

/// A server that answers `answers` (by method and path) and 404 otherwise.
fn server(answers: &[(&str, u16, &str)]) -> (String, Sent) {
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

fn api(base: &str) -> UpdaterApi {
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
    let modules = [
        InstalledModule {
            name: "orchestrator".into(),
            version: "2.0.1".into(),
            drain_path: Some("/internal/drain".into()),
        },
        InstalledModule {
            name: "quality-data".into(),
            version: "1.4.0".into(),
            drain_path: None,
        },
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
            {"name": "orchestrator", "version": "2.0.1", "drain_path": "/internal/drain"},
            {"name": "quality-data", "version": "1.4.0", "drain_path": null},
        ]})
    );
    let (base, _) = server(&[("PUT /updater/v1/modules", 400, r#"{"error":"bad module"}"#)]);
    let err = api(&base).put_modules(&[]).unwrap_err();
    assert!(err.contains("400 bad module"), "{err}");
    // An HTTP error is an answer.
    assert!(!no_answer(&err), "{err}");
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
fn closed() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    base
}
