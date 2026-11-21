//! Which images are pulled, and how `docker` is asked.

use std::cell::RefCell;
use std::collections::BTreeMap;

use updater::state::{Module, Modules};

use super::*;

const HOST: &str = "1.dkr.ecr.us-east-1.amazonaws.com";

fn digest(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

fn offer(images: &[(&str, &str, String)]) -> Offer {
    let mut modules = Modules::new();
    for (module, name, image) in images {
        modules
            .entry(module.to_string())
            .or_insert_with(|| Module {
                version: "1.4.0".into(),
                images: BTreeMap::new(),
            })
            .images
            .insert(name.to_string(), image.clone());
    }
    Offer {
        version: "1.4.0".into(),
        modules,
        packages: Vec::new(),
        notes: String::new(),
        registry: Some((HOST.into(), "AWS".into(), "temporary".into())),
    }
}

#[derive(Default)]
struct Recorder(RefCell<Vec<(String, Vec<String>)>>);
impl ImagePuller for Recorder {
    fn pull(&self, auth: &RegistryAuth, images: &[String]) -> Result<(), String> {
        self.0
            .borrow_mut()
            .push((auth.host.clone(), images.to_vec()));
        Ok(())
    }
}

#[test]
fn every_image_is_pulled_once_from_the_registry() {
    let api = format!(
        "{HOST}/quality-data/quality-data/api@sha256:{}",
        digest('a')
    );
    let server = format!(
        "{HOST}/quality-data/orchestrator/api@sha256:{}",
        digest('b')
    );
    let o = offer(&[
        ("quality-data", "api", api.clone()),
        ("orchestrator", "api", server.clone()),
        // The same image in two modules is pulled once.
        ("orchestrator", "worker", api.clone()),
    ]);
    let r = Recorder::default();
    pull_all(&o, &[], &r).unwrap();
    assert_eq!(*r.0.borrow(), [(HOST.to_string(), vec![server, api])]);
}

#[test]
fn the_agent_s_own_images_are_pulled_with_the_version_s_from_the_registry_only() {
    let api = format!(
        "{HOST}/quality-data/quality-data/api@sha256:{}",
        digest('a')
    );
    let helper = format!("{HOST}/quality-data/tools/busybox@sha256:{}", digest('c'));
    let o = offer(&[("quality-data", "api", api.clone())]);
    let r = Recorder::default();
    pull_all(&o, std::slice::from_ref(&helper), &r).unwrap();
    assert_eq!(*r.0.borrow(), [(HOST.to_string(), vec![api, helper])]);
    let outside = format!("docker.io/library/busybox@sha256:{}", digest('c'));
    let err = pull_all(&o, &[outside], &Recorder::default()).unwrap_err();
    assert!(err.starts_with("the agent: docker.io"), "{err}");
}

#[test]
fn an_image_outside_the_registry_or_not_pinned_is_refused() {
    for image in [
        format!("docker.io/library/trino@sha256:{}", digest('a')),
        format!("{HOST}.evil.example/x@sha256:{}", digest('a')),
        format!("{HOST}/quality-data/x:1.4.0"),
        format!("{HOST}/quality-data/x@sha256:abc"),
    ] {
        let r = Recorder::default();
        let err = pull_all(&offer(&[("m", "i", image.clone())]), &[], &r).unwrap_err();
        assert!(
            err.contains("is not pinned in the installation's registry"),
            "{image}: {err}"
        );
        assert!(r.0.borrow().is_empty(), "nothing is pulled");
    }
    let mut o = offer(&[]);
    o.registry = None;
    assert!(pull_all(&o, &[], &Recorder::default()).is_err());
}

/// A fake `docker` that writes down its arguments and what came on stdin.
fn fake_docker(name: &str, fail_pull: bool) -> (PathBuf, Docker) {
    let dir = std::env::temp_dir().join(format!("agent-docker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("log");
    let pull = if fail_pull {
        "echo 'manifest unknown' >&2; exit 1"
    } else {
        "exit 0"
    };
    let script = format!(
        r#"#!/bin/sh
echo "$@" >> {log}
[ -d "$2" ] || exit 9
case "$3" in
  login) echo "stdin=$(cat)" >> {log} ;;
  pull) {pull} ;;
esac
"#,
        log = log.display()
    );
    let docker = dir.join("docker");
    crate::fake_exec::write(&docker, &script);
    let scratch = dir.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    (dir, Docker { docker, scratch })
}

fn auth() -> RegistryAuth {
    RegistryAuth {
        host: HOST.into(),
        user: "AWS".into(),
        password: "temporary".into(),
    }
}

#[test]
fn docker_logs_in_with_the_password_on_stdin_and_pulls_by_digest() {
    let (dir, docker) = fake_docker("ok", false);
    let image = format!("{HOST}/quality-data/x@sha256:{}", digest('a'));
    {
        let _running = crate::fake_exec::running();
        docker.pull(&auth(), std::slice::from_ref(&image)).unwrap();
    }
    let log = std::fs::read_to_string(dir.join("log")).unwrap();
    let lines: Vec<_> = log.lines().collect();
    assert!(lines[0].ends_with(&format!("login --username AWS --password-stdin {HOST}")));
    assert!(
        !lines[0].contains("temporary"),
        "the password is not an argument"
    );
    assert_eq!(lines[1], "stdin=temporary");
    assert!(lines[2].ends_with(&format!("pull --quiet {image}")));
    // The configuration that held the credential is gone.
    assert_eq!(std::fs::read_dir(&docker.scratch).unwrap().count(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_failed_pull_says_why_and_leaves_no_credential() {
    let (dir, docker) = fake_docker("fail", true);
    let err = {
        let _running = crate::fake_exec::running();
        docker.pull(&auth(), &["x@sha256:1".into()]).unwrap_err()
    };
    assert!(err.contains("manifest unknown"), "{err}");
    assert_eq!(std::fs::read_dir(&docker.scratch).unwrap().count(), 0);
    std::fs::remove_dir_all(dir).unwrap();
}
