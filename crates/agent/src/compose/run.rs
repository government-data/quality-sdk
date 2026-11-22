//! The `Compose` trait of the host on [`DockerCompose`]: each module
//! brought up in its order, waiting for its healthchecks, stopped, and its
//! health.

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
    /// for its healthchecks. Its containers are recreated (decision 0005):
    /// every start follows a version put (an apply, going back, a restore,
    /// the install), and an unchanged container would keep the replaced
    /// version's bind-mounted files.
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
                    "--force-recreate",
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

    /// From the module's `module.yaml`, where its package was put.
    fn drain_path(&self, module: &str) -> Result<Option<String>, String> {
        let m = crate::manifests::read(&self.install, module)?;
        Ok(m.and_then(|m| m.drain).map(|d| d.path))
    }

    /// Every module's health: its containers' Compose healthchecks, the
    /// floor, then what its manifest's `health` adds.
    fn modules_healthy(&self) -> Result<(), String> {
        let r = self.record_or_error()?;
        let mut sick = Vec::new();
        for module in &r.now.order {
            sick.extend(self.sick(module)?);
        }
        if sick.is_empty() {
            Ok(())
        } else {
            Err(sick.join("; "))
        }
    }
}
