//! Every image a platform version names is in the installations' ECR
//! registry, pinned by digest, and is there (decision 0001): `publish`
//! checks it before uploading or signing anything, so no index names an
//! image an installation cannot pull.

use aws_client::ecr::Ecr;
use index_model::PlatformVersion;

/// The installations' registry.
pub trait Registry {
    /// Its host: `<account>.dkr.ecr.<region>.amazonaws.com`.
    fn host(&self) -> Result<String, String>;
    /// Whether `repository` of the registry `registry_id` holds `digest`
    /// (`sha256:<hex>`).
    fn has_image(&self, registry_id: &str, repository: &str, digest: &str) -> Result<bool, String>;
}

impl Registry for Ecr {
    fn host(&self) -> Result<String, String> {
        self.authorization()
            .map(|auth| auth.host)
            .map_err(|e| e.to_string())
    }

    fn has_image(&self, registry_id: &str, repository: &str, digest: &str) -> Result<bool, String> {
        Ecr::has_image(self, registry_id, repository, digest).map_err(|e| e.to_string())
    }
}

/// Every image of `version` in `registry`, by digest; the error names each
/// one that is not.
pub fn check(registry: &dyn Registry, version: &PlatformVersion) -> Result<(), String> {
    let host = registry.host().map_err(|e| format!("ECR: {e}"))?;
    let registry_id = host.split('.').next().unwrap_or_default();
    let mut wrong = Vec::new();
    for (module, m) in &version.modules {
        for (name, image) in &m.images {
            let Some((repository, digest)) = image
                .strip_prefix(&format!("{host}/"))
                .and_then(|rest| rest.split_once('@'))
                .filter(|(repository, _)| !repository.is_empty())
            else {
                wrong.push(format!("{module}/{name}: {image} is not in {host}"));
                continue;
            };
            if !registry
                .has_image(registry_id, repository, digest)
                .map_err(|e| format!("ECR: {e}"))?
            {
                wrong.push(format!("{module}/{name}: {image} is not in its repository"));
            }
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} cannot be published: {}",
            version.version,
            wrong.join("; ")
        ))
    }
}
