//! The `Compose` trait of the host on [`DockerCompose`]: each module
//! brought up in its order, waiting for its healthchecks, stopped, and its
//! health's floor.

use serde::Deserialize;
use updater::engine::Installed;
use updater::state::Modules;

use super::{DockerCompose, Record};

impl DockerCompose {
    fn record_or_error(&self) -> Result<Record, String> {
        self.read_record()?.ok_or_else(|| {
            format!(
                "no platform version is recorded in {}",
                self.record.display()
            )
        })
    }
}

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

impl crate::host::Compose for DockerCompose {
    fn put_version(
        &self,
        version: &str,
        modules: &Modules,
        order: &[String],
    ) -> Result<(), String> {
        DockerCompose::put_version(self, version, modules, order)
    }

    /// The modules the version no longer has go down (their volumes kept),
    /// last started first; then each module comes up in its order, waiting
    /// for its healthchecks.
    fn start(&self) -> Result<(), String> {
        let r = self.record_or_error()?;
        for module in r.stopping.iter().rev() {
            if self.install.join(module).join("current").exists() {
                self.run(module, &["down", "--remove-orphans"])?;
            }
        }
        let wait = self.wait_secs.to_string();
        for module in &r.now.order {
            self.run(
                module,
                &[
                    "up",
                    "--detach",
                    "--no-build",
                    "--remove-orphans",
                    "--wait",
                    "--wait-timeout",
                    &wait,
                ],
            )
            .map_err(|e| format!("{module}: {e}"))?;
        }
        Ok(())
    }

    /// Every module stopped, last started first (to restore the database
    /// with nothing connected).
    fn stop(&self) -> Result<(), String> {
        let Some(r) = self.read_record()? else {
            return Ok(());
        };
        for module in r.now.order.iter().rev() {
            self.run(module, &["stop"])
                .map_err(|e| format!("{module}: {e}"))?;
        }
        Ok(())
    }

    fn installed(&self) -> Result<Installed, String> {
        DockerCompose::installed(self)
    }

    /// The floor of every module's health: each has containers, every one
    /// running, and healthy when it has a healthcheck. The manifest's
    /// `health` is read once the module manifest exists (decision 0002).
    fn modules_healthy(&self) -> Result<(), String> {
        let r = self.record_or_error()?;
        let mut sick = Vec::new();
        for module in &r.now.order {
            let all = containers(&self.run(module, &["ps", "--all", "--format", "json"])?)?;
            if all.is_empty() {
                sick.push(format!("{module} has no container"));
            }
            for c in all {
                if c.state != "running" || !(c.health.is_empty() || c.health == "healthy") {
                    let health = if c.health.is_empty() {
                        String::new()
                    } else {
                        format!(", {}", c.health)
                    };
                    sick.push(format!("{module}/{} is {}{health}", c.service, c.state));
                }
            }
        }
        if sick.is_empty() {
            Ok(())
        } else {
            Err(sick.join("; "))
        }
    }
}
