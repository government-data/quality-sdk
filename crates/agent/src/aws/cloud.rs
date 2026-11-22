//! The AWS adapter as the host's [`Cloud`]: the signed index, the packages,
//! the images and the reports, with the installation's Roles Anywhere
//! credentials (decisions 0001 step 5 and 0003 step 6).

use std::path::PathBuf;
use std::rc::Rc;

use index_model::IndexKey;
use updater::engine::{is_newer, CloudError, Offer, OfferedPackage};
use updater::state::Outcome;

use super::client::{Account, AwsError, Bucket};
use super::credentials::{Cached, SigningHelper};
use super::ecr::Ecr;
use super::http::Https;
use super::images::{pull_all, Docker, ImagePuller};
use super::index::{first, offer, IndexReader};
use super::packages::download;
use super::reports::{Report, FORMAT};
use crate::config::Config;
use crate::host::Cloud;
use crate::paths::Paths;

/// Every AWS piece the agent uses.
pub struct AwsCloud {
    pub installation: String,
    pub bucket: Bucket,
    pub ecr: Ecr,
    pub index: IndexReader,
    pub packages_dir: PathBuf,
    pub puller: Box<dyn ImagePuller>,
    /// The images the agent itself runs, pulled with every version's: the
    /// helper image that empties a volume before a restore.
    pub tools: Vec<String>,
}

impl std::fmt::Debug for AwsCloud {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsCloud")
            .field("installation", &self.installation)
            .field("bucket", &self.bucket)
            .finish_non_exhaustive()
    }
}

impl AwsCloud {
    /// From the settings and the installation's files: the enrolment
    /// certificate and key, and the pinned index key.
    pub fn from_config(config: &Config, paths: &Paths) -> Result<Self, String> {
        let pem = std::fs::read_to_string(paths.index_key())
            .map_err(|e| format!("the pinned index key {}: {e}", paths.index_key().display()))?;
        let key = IndexKey::from_public_key_pem(&pem).map_err(|e| e.to_string())?;
        let helper = SigningHelper {
            helper: config.aws.signing_helper.clone(),
            certificate: paths.certificate(),
            private_key: paths.private_key(),
            trust_anchor_arn: config.aws.trust_anchor_arn.clone(),
            profile_arn: config.aws.profile_arn.clone(),
            role_arn: config.aws.role_arn.clone(),
            region: config.aws.region.clone(),
        };
        let account = Rc::new(Account::new(
            &config.aws.region,
            Box::new(Cached::new(helper)),
            Box::new(Https::default()),
        ));
        Ok(Self {
            installation: config.installation.clone(),
            bucket: Bucket::new(account.clone(), &config.aws.bucket),
            ecr: Ecr::new(account),
            index: IndexReader {
                channel: config.channel,
                key,
                seen: paths.index_seen(),
            },
            packages_dir: paths.packages_dir(),
            puller: Box::new(Docker::default()),
            tools: vec![config.helper_image()],
        })
    }
}

impl AwsCloud {
    /// The first installation's version from the verified index
    /// ([`first`]), with the registry's credential to pull its images.
    pub fn choose(&self, version: Option<&str>) -> Result<Offer, CloudError> {
        let index = self.index.read(&self.bucket)?;
        let mut o = first(&index, version, &self.installation)?;
        let auth = self.ecr.authorization().map_err(registry)?;
        o.registry = Some((auth.host, auth.user, auth.password));
        Ok(o)
    }

    /// A report of kind `install`, or `install_failed` with where it
    /// stopped and why; if it cannot be written, it does not matter.
    pub fn report_install(&self, version: &str, failed: Option<&str>) {
        let report = Report {
            format: FORMAT,
            installation: &self.installation,
            agent: env!("CARGO_PKG_VERSION"),
            kind: if failed.is_some() {
                "install_failed"
            } else {
                "install"
            },
            version,
            from: "",
            at: chrono::Utc::now(),
            detail: failed.unwrap_or(""),
        };
        match report.write(&self.bucket) {
            Ok(key) => tracing::info!("reported {key}"),
            Err(err) => tracing::warn!("could not report the install: {err}"),
        }
    }
}

fn registry(e: AwsError) -> CloudError {
    match e {
        AwsError::Denied(m) => CloudError::Rejected(m),
        AwsError::Unavailable(m) | AwsError::Failed(m) => CloudError::Network(m),
    }
}

impl Cloud for AwsCloud {
    /// The verified index's offer; with a newer version, the registry's
    /// credential to pull its images.
    fn check(&self, installed_version: &str) -> Result<Offer, CloudError> {
        let index = self.index.read(&self.bucket)?;
        let mut o = offer(&index, installed_version, &self.installation)?;
        if is_newer(&o.version, installed_version) {
            let auth = self.ecr.authorization().map_err(registry)?;
            o.registry = Some((auth.host, auth.user, auth.password));
        }
        Ok(o)
    }

    /// If it cannot be written, it does not matter: the update is done.
    fn report(&self, outcome: &Outcome) {
        match Report::of(&self.installation, outcome).write(&self.bucket) {
            Ok(key) => tracing::info!("reported {key}"),
            Err(err) => tracing::warn!("could not report the outcome: {err}"),
        }
    }

    /// The package of the module at its version in the offer.
    fn download_package(&self, o: &Offer, p: &OfferedPackage) -> Result<PathBuf, String> {
        let version = o
            .modules
            .get(&p.module)
            .map_or(o.version.as_str(), |m| m.version.as_str());
        download(&self.bucket, p, version, &self.packages_dir)
    }

    fn pull_images(&self, o: &Offer) -> Result<(), String> {
        pull_all(o, &self.tools, self.puller.as_ref())
    }
}

#[cfg(test)]
#[path = "cloud_tests.rs"]
mod tests;
