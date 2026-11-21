//! The installation's AWS credentials: its enrolment certificate traded for
//! one-hour credentials of the installation role through IAM Roles Anywhere
//! (decision 0001). `aws_signing_helper credential-process` does the trade,
//! as `tools/verify/roles-anywhere.sh` verified; no long-lived secret is kept,
//! and the credentials live only in memory.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

/// Temporary credentials. Never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub expiration: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("expiration", &self.expiration)
            .finish_non_exhaustive()
    }
}

/// Where credentials come from.
pub trait CredentialSource {
    fn credentials(&self) -> Result<Credentials, String>;
}

/// `aws_signing_helper credential-process` with the enrolment certificate.
#[derive(Debug, Clone)]
pub struct SigningHelper {
    pub helper: PathBuf,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
    pub trust_anchor_arn: String,
    pub profile_arn: String,
    pub role_arn: String,
    pub region: String,
}

/// What `credential-process` prints (the AWS CLI's process credentials).
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Process {
    version: u32,
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    expiration: Option<DateTime<Utc>>,
}

impl CredentialSource for SigningHelper {
    fn credentials(&self) -> Result<Credentials, String> {
        let output = Command::new(&self.helper)
            .arg("credential-process")
            .arg("--certificate")
            .arg(&self.certificate)
            .arg("--private-key")
            .arg(&self.private_key)
            .args(["--trust-anchor-arn", &self.trust_anchor_arn])
            .args(["--profile-arn", &self.profile_arn])
            .args(["--role-arn", &self.role_arn])
            .args(["--region", &self.region])
            .output()
            .map_err(|e| format!("could not run {}: {e}", self.helper.display()))?;
        if !output.status.success() {
            // Its error says why (a revoked or expired certificate…); what it
            // prints on success holds credentials and is never shown.
            let why = String::from_utf8_lossy(&output.stderr);
            let why: String = why.trim().chars().take(300).collect();
            return Err(format!("Roles Anywhere refused the certificate: {why}"));
        }
        parse(&output.stdout)
    }
}

fn parse(stdout: &[u8]) -> Result<Credentials, String> {
    let p: Process = serde_json::from_slice(stdout)
        .map_err(|_| "the signing helper printed no credentials".to_string())?;
    if p.version != 1 {
        return Err(format!("credential-process version {} is not 1", p.version));
    }
    Ok(Credentials {
        access_key_id: p.access_key_id,
        secret_access_key: p.secret_access_key,
        session_token: p.session_token,
        expiration: p.expiration,
    })
}

/// Credentials kept until five minutes before they expire.
#[derive(Debug)]
pub struct Cached<S> {
    source: S,
    kept: Mutex<Option<Credentials>>,
}

/// How long before expiring credentials are renewed.
const MARGIN: Duration = Duration::minutes(5);

impl<S: CredentialSource> Cached<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            kept: Mutex::new(None),
        }
    }
}

impl<S: CredentialSource> CredentialSource for Cached<S> {
    fn credentials(&self) -> Result<Credentials, String> {
        let mut kept = self
            .kept
            .lock()
            .map_err(|_| "credentials lock poisoned".to_string())?;
        let fresh = |c: &Credentials| c.expiration.is_some_and(|e| e - MARGIN > Utc::now());
        if let Some(c) = kept.as_ref().filter(|c| fresh(c)) {
            return Ok(c.clone());
        }
        let c = self.source.credentials()?;
        *kept = Some(c.clone());
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn helper(name: &str, script: &str) -> (PathBuf, SigningHelper) {
        let dir = std::env::temp_dir().join(format!("agent-helper-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aws_signing_helper");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let h = SigningHelper {
            helper: path,
            certificate: "/etc/quality-data/enrolment/certificate.pem".into(),
            private_key: "/etc/quality-data/enrolment/private-key.pem".into(),
            trust_anchor_arn: "arn:aws:rolesanywhere:us-east-1:1:trust-anchor/t".into(),
            profile_arn: "arn:aws:rolesanywhere:us-east-1:1:profile/p".into(),
            role_arn: "arn:aws:iam::1:role/r".into(),
            region: "us-east-1".into(),
        };
        (dir, h)
    }

    #[test]
    fn the_helper_is_called_with_the_certificate_and_its_answer_read() {
        // It answers only when called with what the profile needs.
        let (dir, h) = helper(
            "ok",
            r#"[ "$1" = credential-process ] && [ "$3" = /etc/quality-data/enrolment/certificate.pem ] \
  && [ "$7" = arn:aws:rolesanywhere:us-east-1:1:trust-anchor/t ] && [ "${13}" = us-east-1 ] || exit 3
echo '{"Version":1,"AccessKeyId":"ASIAX","SecretAccessKey":"s","SessionToken":"t","Expiration":"2026-11-21T12:00:00Z"}'"#,
        );
        let c = h.credentials().unwrap();
        assert_eq!(c.access_key_id, "ASIAX");
        assert_eq!(c.session_token.as_deref(), Some("t"));
        assert_eq!(
            c.expiration.unwrap().to_rfc3339(),
            "2026-11-21T12:00:00+00:00"
        );
        assert!(!format!("{c:?}").contains("ASIAX"), "never printed");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_refusal_says_why_and_never_shows_what_it_printed() {
        let (dir, h) = helper(
            "refused",
            "echo secret-output; echo 'AccessDeniedException: revoked' >&2; exit 1",
        );
        let err = h.credentials().unwrap_err();
        assert!(err.contains("AccessDeniedException: revoked"), "{err}");
        assert!(!err.contains("secret-output"), "{err}");
        let (dir2, mut h) = helper("garbage", "echo not-json");
        assert!(h.credentials().unwrap_err().contains("no credentials"));
        h.helper = dir2.join("missing");
        assert!(h.credentials().unwrap_err().contains("could not run"));
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(dir2).unwrap();
    }

    struct Counting(Cell<u32>, Option<DateTime<Utc>>);
    impl CredentialSource for Counting {
        fn credentials(&self) -> Result<Credentials, String> {
            self.0.set(self.0.get() + 1);
            Ok(Credentials {
                access_key_id: "a".into(),
                secret_access_key: "s".into(),
                session_token: None,
                expiration: self.1,
            })
        }
    }

    #[test]
    fn credentials_are_kept_until_shortly_before_they_expire() {
        let hour = Cached::new(Counting(
            Cell::new(0),
            Some(Utc::now() + Duration::hours(1)),
        ));
        hour.credentials().unwrap();
        hour.credentials().unwrap();
        assert_eq!(hour.source.0.get(), 1);
        let soon = Cached::new(Counting(
            Cell::new(0),
            Some(Utc::now() + Duration::minutes(2)),
        ));
        soon.credentials().unwrap();
        soon.credentials().unwrap();
        assert_eq!(soon.source.0.get(), 2);
    }
}
