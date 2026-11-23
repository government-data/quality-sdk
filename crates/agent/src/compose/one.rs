//! One module at a time, for `qd module add|remove` (decision 0002): brought
//! up or down alone, its health, and every installed module's route
//! written again.

use super::DockerCompose;

impl DockerCompose {
    /// The installed modules in their start order.
    pub fn order(&self) -> Result<Vec<String>, String> {
        Ok(self.read_record()?.map(|r| r.now.order).unwrap_or_default())
    }

    /// `module` up, its containers recreated, waiting for its healthchecks
    /// (decision 0005).
    pub fn up_one(&self, module: &str) -> Result<(), String> {
        let wait = self.wait_secs.to_string();
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
        .map(|_| ())
        .map_err(|e| format!("{module}: {e}"))
    }

    /// `module` down, its volumes kept.
    pub fn down_one(&self, module: &str) -> Result<(), String> {
        self.run(module, &["down", "--remove-orphans"])
            .map(|_| ())
            .map_err(|e| format!("{module}: {e}"))
    }

    /// `module`'s health: its containers' healthchecks and its manifest's.
    pub fn healthy_one(&self, module: &str) -> Result<(), String> {
        let sick = self.sick(module)?;
        if sick.is_empty() {
            Ok(())
        } else {
            Err(sick.join("; "))
        }
    }

    /// Every installed module's route written from its manifest, the
    /// others' removed ([`crate::routes::sync`]).
    pub fn sync_routes(&self) -> Result<(), String> {
        let installed = self
            .order()?
            .into_iter()
            .map(|m| {
                let manifest = crate::manifests::read(&self.install, &m)?;
                Ok((m, manifest))
            })
            .collect::<Result<Vec<_>, String>>()?;
        crate::routes::sync(&self.routes, &installed)
    }
}
