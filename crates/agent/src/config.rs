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
//!
//! [api]                        # optional: quality-data's `updater/v1`
//! url = "https://quality-data.internal:9443/api"   # its `internal` entry point
//! connect = "127.0.0.1:9443"   # optional: where to connect; the URL's host
//!                              # stays the TLS name and the Host header
//! ```
//!
//! It holds no secret: the credentials come from the enrolment certificate,
//! whose key lives beside it ([`crate::paths::Paths`]).

use std::net::SocketAddr;
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
    /// quality-data's `updater/v1`; without it the agent tells the API
    /// nothing (commands such as `check` do not need it).
    #[serde(default)]
    pub api: Option<ApiSettings>,
}

/// Where quality-data serves `updater/v1`: its `internal` entry point, with
/// mTLS (the certificates are at [`crate::paths::Paths`]).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSettings {
    /// `https://host[:port][/path]`: the entry point and the path the API
    /// is served under (`/api` behind Traefik).
    pub url: String,
    /// Where to connect instead of resolving the URL's host (an IP address
    /// and a port): the host's own Traefik for `INTERNAL_DOMAIN`.
    #[serde(default)]
    pub connect: Option<SocketAddr>,
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
        check_installation(&self.installation)?;
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
        if let Some(api) = &self.api {
            let rest = api.url.strip_prefix("https://").unwrap_or_default();
            let host = rest.split('/').next().unwrap_or_default();
            let bad = |c: char| c.is_whitespace() || matches!(c, '?' | '#' | '@' | '\\');
            if host.is_empty() || rest.contains(bad) {
                return Err(format!(
                    "api.url {:?} is not https://host[:port][/path]",
                    api.url
                ));
            }
        }
        Ok(())
    }
}

/// An installation's id, as its certificate's CN and its reports' prefix
/// name it: 1 to 64 letters, digits, `-`, `_` or `.`, not starting with `.`.
pub fn check_installation(id: &str) -> Result<(), String> {
    let safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    if id.is_empty() || id.len() > 64 || !id.chars().all(safe) || id.starts_with('.') {
        return Err(format!(
            "installation {id:?}: 1 to 64 letters, digits, '-', '_' or '.'"
        ));
    }
    Ok(())
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
    fn the_api_is_optional_and_only_https() {
        assert_eq!(Config::parse(EXAMPLE).unwrap().api, None);
        let with = format!("{EXAMPLE}\n[api]\nurl = \"https://qd.internal:8443\"\n");
        let api = Config::parse(&with).unwrap().api.unwrap();
        assert_eq!(api.url, "https://qd.internal:8443");
        for bad in [
            "http://qd.internal",
            "https://",
            "https:///api",
            "https://qd/api?x=1",
            "https://user@qd/api",
            "https://qd/a b",
        ] {
            let text = format!("{EXAMPLE}\n[api]\nurl = \"{bad}\"\n");
            assert!(
                Config::parse(&text).unwrap_err().contains("api.url"),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_api_url_may_carry_a_path_and_a_place_to_connect() {
        let with = format!(
            "{EXAMPLE}\n[api]\nurl = \"https://qd.internal:9443/api\"\nconnect = \"127.0.0.1:9443\"\n"
        );
        let api = Config::parse(&with).unwrap().api.unwrap();
        assert_eq!(api.url, "https://qd.internal:9443/api");
        assert_eq!(api.connect, Some("127.0.0.1:9443".parse().unwrap()));
        let v6 = with.replace("127.0.0.1:9443", "[::1]:9443");
        assert!(Config::parse(&v6).unwrap().api.unwrap().connect.is_some());
        for bad in ["qd.internal:9443", "127.0.0.1", "127.0.0.1:x"] {
            let text = with.replace("127.0.0.1:9443", bad);
            assert!(
                Config::parse(&text).unwrap_err().contains("connect"),
                "{bad}"
            );
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
