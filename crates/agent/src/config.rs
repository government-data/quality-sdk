//! The agent's settings: `/etc/quality-data/agent.toml` (decision 0003).
//!
//! ```toml
//! installation = "inst-0001"   # the enrolment certificate's CN
//! channel = "stable"           # or "patch"
//!
//! [aws]
//! region = "us-east-1"
//! bucket = "quality-data-releases-123456789012"
//! trust_anchor_arn = "arn:aws:rolesanywhere:us-east-1:123456789012:trust-anchor/..."
//! profile_arn = "arn:aws:rolesanywhere:us-east-1:123456789012:profile/..."
//! role_arn = "arn:aws:iam::123456789012:role/quality-data-installation"
//! signing_helper = "/usr/local/bin/aws_signing_helper"   # optional
//! ```
//!
//! It holds no secret: the credentials come from the enrolment certificate,
//! whose key lives beside it ([`crate::paths::Paths`]).

use std::path::{Path, PathBuf};

use index_model::Channel;
use serde::Deserialize;

/// Where `aws_signing_helper` is looked for when the file does not say.
pub const SIGNING_HELPER: &str = "/usr/local/bin/aws_signing_helper";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The installation's id: its enrolment certificate's CN, and the
    /// prefix it reports under (`reports/<id>/`).
    pub installation: String,
    pub channel: Channel,
    pub aws: Aws,
}

/// The AWS side (decision 0001), as Terraform outputs it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aws {
    pub region: String,
    /// The release bucket.
    pub bucket: String,
    pub trust_anchor_arn: String,
    pub profile_arn: String,
    pub role_arn: String,
    #[serde(default = "signing_helper")]
    pub signing_helper: PathBuf,
}

fn signing_helper() -> PathBuf {
    SIGNING_HELPER.into()
}

impl Config {
    /// Read and check the file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parse and check a file's text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        config.check()?;
        Ok(config)
    }

    fn check(&self) -> Result<(), String> {
        let id = &self.installation;
        let safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
        if id.is_empty() || id.len() > 64 || !id.chars().all(safe) || id.starts_with('.') {
            return Err(format!(
                "installation {id:?}: 1 to 64 letters, digits, '-', '_' or '.'"
            ));
        }
        let aws = &self.aws;
        if aws.region.is_empty()
            || !aws
                .region
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(format!("aws.region {:?} is not a region", aws.region));
        }
        let bucket_char =
            |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.';
        if aws.bucket.len() < 3 || !aws.bucket.chars().all(bucket_char) {
            return Err(format!("aws.bucket {:?} is not a bucket name", aws.bucket));
        }
        for (name, arn, service) in [
            (
                "aws.trust_anchor_arn",
                &aws.trust_anchor_arn,
                "rolesanywhere",
            ),
            ("aws.profile_arn", &aws.profile_arn, "rolesanywhere"),
            ("aws.role_arn", &aws.role_arn, "iam"),
        ] {
            if !arn.starts_with(&format!("arn:aws:{service}:")) {
                return Err(format!("{name} is not an {service} ARN"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const EXAMPLE: &str = r#"
installation = "inst-0001"
channel = "stable"

[aws]
region = "us-east-1"
bucket = "quality-data-releases-123456789012"
trust_anchor_arn = "arn:aws:rolesanywhere:us-east-1:123456789012:trust-anchor/a"
profile_arn = "arn:aws:rolesanywhere:us-east-1:123456789012:profile/p"
role_arn = "arn:aws:iam::123456789012:role/quality-data-installation"
"#;

    #[test]
    fn a_complete_file_is_read() {
        let c = Config::parse(EXAMPLE).unwrap();
        assert_eq!(c.installation, "inst-0001");
        assert_eq!(c.channel, Channel::Stable);
        assert_eq!(c.aws.region, "us-east-1");
        assert_eq!(c.aws.signing_helper, Path::new(SIGNING_HELPER));
    }

    #[test]
    fn a_missing_or_unknown_key_is_refused() {
        let missing = EXAMPLE.replace("channel = \"stable\"\n", "");
        assert!(Config::parse(&missing).unwrap_err().contains("channel"));
        let unknown = EXAMPLE.replace("[aws]", "licence = \"x\"\n[aws]");
        assert!(Config::parse(&unknown).unwrap_err().contains("licence"));
        let channel = EXAMPLE.replace("\"stable\"", "\"nightly\"");
        assert!(Config::parse(&channel).is_err());
    }

    #[test]
    fn values_that_cannot_be_right_are_refused() {
        for (from, to, says) in [
            ("\"inst-0001\"", "\"../other\"", "installation"),
            ("\"inst-0001\"", "\"\"", "installation"),
            ("\"us-east-1\"", "\"us east\"", "aws.region"),
            (
                "\"quality-data-releases-123456789012\"",
                "\"Bucket\"",
                "aws.bucket",
            ),
            ("arn:aws:iam::", "arn:aws:sts::", "aws.role_arn"),
            (
                "arn:aws:rolesanywhere:us-east-1:123456789012:profile",
                "x",
                "aws.profile_arn",
            ),
        ] {
            let err = Config::parse(&EXAMPLE.replace(from, to)).unwrap_err();
            assert!(err.contains(says), "{from} -> {to}: {err}");
        }
    }

    #[test]
    fn it_is_loaded_from_its_file() {
        let dir = std::env::temp_dir().join(format!("agent-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.toml");
        assert!(Config::load(&path).unwrap_err().contains("could not read"));
        std::fs::write(&path, EXAMPLE).unwrap();
        assert_eq!(
            Config::load(&path).unwrap(),
            Config::parse(EXAMPLE).unwrap()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
