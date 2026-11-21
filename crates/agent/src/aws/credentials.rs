//! The installation's AWS credentials: its enrolment certificate traded for
//! one-hour credentials of the installation role through IAM Roles Anywhere
//! (decision 0001). `aws_signing_helper credential-process` does the trade,
//! as `tools/verify/roles-anywhere.sh` verified; no long-lived secret is kept,
//! and the credentials live only in memory.

use std::path::PathBuf;
use std::process::Command;

pub use aws_client::credentials::{parse_process, Cached, CredentialSource, Credentials};

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
        parse_process(&output.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_exec;

    fn helper(name: &str, script: &str) -> (PathBuf, SigningHelper) {
        let dir = std::env::temp_dir().join(format!("agent-helper-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aws_signing_helper");
        fake_exec::write(&path, &format!("#!/bin/sh\n{script}\n"));
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
        let c = {
            let _running = fake_exec::running();
            h.credentials().unwrap()
        };
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
        let (dir2, mut garbage) = helper("garbage", "echo not-json");
        let _running = fake_exec::running();
        let err = h.credentials().unwrap_err();
        assert!(err.contains("AccessDeniedException: revoked"), "{err}");
        assert!(!err.contains("secret-output"), "{err}");
        assert!(garbage
            .credentials()
            .unwrap_err()
            .contains("no credentials"));
        garbage.helper = dir2.join("missing");
        assert!(garbage.credentials().unwrap_err().contains("could not run"));
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(dir2).unwrap();
    }
}
