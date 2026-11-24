//! The AWS adapter composed, against a fake AWS: no test calls a real
//! account.

use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine;
use chrono::Utc;
use sha2::{Digest, Sha256};
use updater::state::OutcomeKind;

use super::super::ecr::RegistryAuth;
use super::super::index::tests::{index, reader, serving};
use super::*;
use aws_client::fake::{account, FakeAws};

/// Remembers what it was asked to pull.
#[derive(Clone, Default)]
struct Recorder(Rc<RefCell<Vec<String>>>);
impl ImagePuller for Recorder {
    fn pull(&self, _auth: &RegistryAuth, images: &[String]) -> Result<(), String> {
        self.0.borrow_mut().extend(images.iter().cloned());
        Ok(())
    }
}

fn cloud(aws: &FakeAws, name: &str) -> (AwsCloud, Recorder) {
    let account = account(aws);
    let index = reader(name);
    let packages_dir = index.seen.parent().unwrap().join("packages");
    let recorder = Recorder::default();
    let cloud = AwsCloud {
        installation: "inst-0001".into(),
        bucket: Bucket::new(account.clone(), "releases"),
        ecr: Ecr::new(account),
        index,
        packages_dir,
        puller: Box::new(recorder.clone()),
        tools: vec![format!(
            "1.dkr.ecr.us-east-1.amazonaws.com/quality-data/tools/busybox@sha256:{}",
            "c".repeat(64)
        )],
    };
    (cloud, recorder)
}

fn cleanup(c: &AwsCloud) {
    let _ = std::fs::remove_dir_all(c.index.seen.parent().unwrap());
}

/// The fake registry's answer.
fn ecr_answers(aws: &FakeAws) {
    let token = base64::engine::general_purpose::STANDARD.encode("AWS:temporary");
    let body = format!(
        r#"{{"authorizationData":[{{"authorizationToken":"{token}","proxyEndpoint":"https://1.dkr.ecr.us-east-1.amazonaws.com"}}]}}"#
    );
    aws.answer(
        "POST https://api.ecr.us-east-1.amazonaws.com/",
        200,
        body.as_bytes(),
    );
}

#[test]
fn a_newer_version_comes_with_the_registry_and_is_prepared_through_aws() {
    let aws = serving(&index("stable", 3), 1);
    ecr_answers(&aws);
    let (c, pulled) = cloud(&aws, "cloud-newer");
    let o = c.check("1.2.0").unwrap();
    assert_eq!(o.version, "1.4.0");
    let (host, user, password) = o.registry.clone().unwrap();
    assert_eq!(host, "1.dkr.ecr.us-east-1.amazonaws.com");
    assert_eq!((user.as_str(), password.as_str()), ("AWS", "temporary"));

    // The orchestrator's package, at its module version, checked.
    let body = b"orchestrator package";
    let mut p = o.packages[0].clone();
    p.sha256 = hex::encode(Sha256::digest(body));
    aws.answer("GET /packages/orchestrator/1.4.0.tar.gz", 200, body);
    let path = c.download_package(&o, &p).unwrap();
    assert!(
        path.ends_with("packages/orchestrator/1.4.0.tar.gz"),
        "{}",
        path.display()
    );

    c.pull_images(&o).unwrap();
    // The version's two images and the helper image.
    assert_eq!(pulled.0.borrow().len(), 3);
    cleanup(&c);
}

#[test]
fn up_to_date_the_registry_is_not_asked() {
    let aws = serving(&index("stable", 3), 1);
    let (c, _) = cloud(&aws, "cloud-current");
    let o = c.check("1.4.0").unwrap();
    assert_eq!(o.version, "1.4.0");
    assert_eq!(o.registry, None);
    assert!(!aws.sent.borrow().iter().any(|r| r.url.contains("api.ecr")));
    cleanup(&c);
}

#[test]
fn a_denied_registry_is_a_refusal() {
    let aws = serving(&index("stable", 3), 1);
    aws.answer("POST /", 400, br#"{"__type":"AccessDeniedException"}"#);
    let (c, _) = cloud(&aws, "cloud-ecr-denied");
    assert!(matches!(c.check("1.2.0"), Err(CloudError::Rejected(_))));
    cleanup(&c);
}

#[test]
fn an_outcome_is_reported_and_a_failed_report_does_not_matter() {
    let aws = FakeAws::default();
    aws.answer("PUT /reports/inst-0001/", 200, b"");
    let (c, _) = cloud(&aws, "cloud-report");
    let outcome = Outcome {
        kind: OutcomeKind::Updated,
        version: "1.4.0".into(),
        from: "1.2.0".into(),
        detail: "updated from 1.2.0 to 1.4.0".into(),
        at: Utc::now(),
    };
    // The fake answers any key ending in the prefix only: this one fails,
    // quietly.
    c.report(&outcome);
    let sent = aws.sent.borrow();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].url.contains("/reports/inst-0001/"),
        "{}",
        sent[0].url
    );
    assert!(sent[0].url.ends_with("-updated-1.4.0.json"));
    cleanup(&c);
}

#[test]
fn a_refusal_for_a_replica_not_in_sync_is_reported_as_replica_refused() {
    let aws = FakeAws::default();
    let (c, _) = cloud(&aws, "cloud-replica-refused");
    let outcome = Outcome {
        kind: OutcomeKind::Failed,
        version: "1.4.0".into(),
        from: "1.2.0".into(),
        detail: "not updated: lake-rustfs's volume rustfs-data has no replica in sync (behind)"
            .into(),
        at: Utc::now(),
    };
    c.report_replica_refused(&outcome);
    let sent = aws.sent.borrow();
    assert!(
        sent[0].url.contains("/reports/inst-0001/")
            && sent[0].url.ends_with("-replica_refused-1.4.0.json"),
        "{}",
        sent[0].url
    );
    let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body["kind"], "replica_refused");
    assert_eq!(body["detail"], outcome.detail);
    assert!(body["channel"].is_string(), "{body}");
    cleanup(&c);
}

#[test]
fn a_first_installation_chooses_with_the_registry_and_reports_its_install() {
    let aws = serving(&index("stable", 3), 1);
    ecr_answers(&aws);
    aws.answer("PUT /reports/inst-0001/", 200, b"");
    let (c, _) = cloud(&aws, "cloud-install");
    let o = c.choose(Some("1.3.0")).unwrap();
    assert_eq!(o.version, "1.3.0");
    assert!(o.registry.is_some());
    c.report_install("1.3.0", None);
    c.report_install("1.3.0", Some("the install stopped at start: no"));
    let sent = aws.sent.borrow();
    let reports: Vec<_> = sent
        .iter()
        .filter(|r| r.url.contains("/reports/"))
        .collect();
    assert!(reports[0].url.ends_with("-install-1.3.0.json"));
    assert!(reports[1].url.ends_with("-install_failed-1.3.0.json"));
    let body = String::from_utf8(reports[1].body.clone()).unwrap();
    assert!(body.contains("stopped at start"), "{body}");
    cleanup(&c);
}
