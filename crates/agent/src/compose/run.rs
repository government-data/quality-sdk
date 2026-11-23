//! The `Compose` trait of the host on [`DockerCompose`]: each module
//! brought up in its order, waiting for its healthchecks, stopped, and its
//! health.

use module_manifest::Manifest;
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
    /// version's bind-mounted files. Then each module's route is written
    /// from its manifest and the routes of modules no longer there removed.
    fn start(&self) -> Result<(), String> {
        let r = self.record_or_error()?;
        for module in r.stopping.iter().rev() {
            if self.install.join(module).join("current").exists() {
                self.run(module, &["down", "--remove-orphans"])?;
            }
        }
        for module in &r.now.order {
            self.up_one(module)?;
        }
        self.sync_routes()
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

    /// The module's `module.yaml`, where its package was put.
    fn manifest(&self, module: &str) -> Result<Option<Manifest>, String> {
        crate::manifests::read(&self.install, module)
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
