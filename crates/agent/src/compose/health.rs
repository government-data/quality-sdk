//! A module's health: the floor, its containers' Compose healthchecks, and
//! what its manifest's `health` adds (decision 0002), as
//! `tools/verify/compose.sh` and `tools/verify/module-data.sh` verified:
//!
//! - every module has containers, each `running` and, with a healthcheck,
//!   `healthy` (`ps --all --format json`);
//! - each service `health.services` names has a container;
//! - each of `health.checks` succeeds: `exec -T <service> <command>`, ended
//!   by `timeout` after its `timeout_secs`.

use module_manifest::Health;
use serde::Deserialize;

use super::DockerCompose;
use crate::command::Cmd;

/// One container as `docker compose ps --all --format json` says it.
#[derive(Debug, Deserialize)]
struct Container {
    #[serde(rename = "Service", default)]
    service: String,
    #[serde(rename = "State", default)]
    state: String,
    #[serde(rename = "Health", default)]
    health: String,
}

/// The containers of `ps --format json`: one object per line (as Compose
/// 2.21 and later print them), or one array (as earlier ones did).
fn containers(output: &str) -> Result<Vec<Container>, String> {
    let output = output.trim();
    if output.starts_with('[') {
        return serde_json::from_str(output).map_err(|e| format!("docker compose ps: {e}"));
    }
    output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("docker compose ps: {e}")))
        .collect()
}

/// How long `timeout` waits after its signal before it kills the check.
const KILL_AFTER: &str = "--kill-after=5";

impl DockerCompose {
    /// What is wrong with `module`: empty when it is healthy.
    pub(super) fn sick(&self, module: &str) -> Result<Vec<String>, String> {
        let all = containers(&self.run(module, &["ps", "--all", "--format", "json"])?)?;
        let mut sick = Vec::new();
        if all.is_empty() {
            sick.push(format!("{module} has no container"));
        }
        for c in &all {
            if c.state != "running" || !(c.health.is_empty() || c.health == "healthy") {
                let health = if c.health.is_empty() {
                    String::new()
                } else {
                    format!(", {}", c.health)
                };
                sick.push(format!("{module}/{} is {}{health}", c.service, c.state));
            }
        }
        match crate::manifests::read(&self.install, module) {
            Ok(Some(m)) => sick.extend(self.manifest_sick(module, &m.health, &all)),
            Ok(None) => {}
            Err(e) => sick.push(e),
        }
        Ok(sick)
    }

    fn manifest_sick(&self, module: &str, health: &Health, all: &[Container]) -> Vec<String> {
        let mut sick = Vec::new();
        for s in &health.services {
            if !all.iter().any(|c| &c.service == s) {
                sick.push(format!("{module}/{s} has no container"));
            }
        }
        for check in &health.checks {
            let mut args = vec!["exec", "-T", check.service.as_str()];
            args.extend(check.command.iter().map(String::as_str));
            let run = self.compose(module, &args).and_then(|cmd| {
                let secs = check.timeout_secs.to_string();
                let mut all = vec![KILL_AFTER.to_string(), secs, cmd.program];
                all.extend(cmd.args);
                self.runner.run(&Cmd::new("timeout", all))
            });
            if let Err(e) = run {
                sick.push(format!(
                    "{module}/{}: the check {} failed: {e}",
                    check.service, check.command[0]
                ));
            }
        }
        sick
    }
}
