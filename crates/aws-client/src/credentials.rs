//! AWS credentials, kept only in memory: where they come from is a port
//! ([`CredentialSource`]), and [`Cached`] keeps them until shortly before
//! they expire. [`parse_process`] reads the AWS CLI's process credentials
//! (version 1), what `aws_signing_helper credential-process` prints.

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

/// The credentials in the environment (`AWS_ACCESS_KEY_ID`,
/// `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`), as GitHub's OIDC login or
/// `aws configure export-credentials --format env` leaves them.
#[derive(Debug, Clone, Copy, Default)]
pub struct Environment;

impl CredentialSource for Environment {
    fn credentials(&self) -> Result<Credentials, String> {
        from_variables(|name| std::env::var(name).ok())
    }
}

/// Credentials from variables, by name.
fn from_variables(var: impl Fn(&str) -> Option<String>) -> Result<Credentials, String> {
    let get = |name: &str| var(name).filter(|v| !v.trim().is_empty());
    match (get("AWS_ACCESS_KEY_ID"), get("AWS_SECRET_ACCESS_KEY")) {
        (Some(access_key_id), Some(secret_access_key)) => Ok(Credentials {
            access_key_id,
            secret_access_key,
            session_token: get("AWS_SESSION_TOKEN"),
            expiration: None,
        }),
        _ => Err(
            "no AWS credentials in the environment: set AWS_ACCESS_KEY_ID and \
                  AWS_SECRET_ACCESS_KEY (and AWS_SESSION_TOKEN), for example with \
                  `eval \"$(aws configure export-credentials --format env)\"`"
                .into(),
        ),
    }
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

/// Process credentials (version 1) from what a credential process printed.
pub fn parse_process(stdout: &[u8]) -> Result<Credentials, String> {
    let p: Process = serde_json::from_slice(stdout)
        .map_err(|_| "the credential process printed no credentials".to_string())?;
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

    use super::*;

    #[test]
    fn process_credentials_are_read_and_never_printed() {
        let c = parse_process(
            br#"{"Version":1,"AccessKeyId":"ASIAX","SecretAccessKey":"s","SessionToken":"t","Expiration":"2026-11-21T12:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(c.access_key_id, "ASIAX");
        assert_eq!(c.session_token.as_deref(), Some("t"));
        assert!(!format!("{c:?}").contains("ASIAX"), "never printed");
        assert!(parse_process(b"not-json")
            .unwrap_err()
            .contains("no credentials"));
        let v2 = br#"{"Version":2,"AccessKeyId":"a","SecretAccessKey":"s"}"#;
        assert!(parse_process(v2).unwrap_err().contains("version 2"));
    }
    #[test]
    fn environment_credentials_need_a_key_and_its_secret() {
        let vars = |pairs: &'static [(&str, &str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_string())
            }
        };
        let c = from_variables(vars(&[
            ("AWS_ACCESS_KEY_ID", "ASIAX"),
            ("AWS_SECRET_ACCESS_KEY", "s"),
            ("AWS_SESSION_TOKEN", "t"),
        ]))
        .unwrap();
        assert_eq!(
            (c.access_key_id.as_str(), c.session_token.as_deref()),
            ("ASIAX", Some("t"))
        );
        let no_token = from_variables(vars(&[
            ("AWS_ACCESS_KEY_ID", "A"),
            ("AWS_SECRET_ACCESS_KEY", "s"),
        ]));
        assert_eq!(no_token.unwrap().session_token, None);
        let err = from_variables(vars(&[("AWS_ACCESS_KEY_ID", "A")])).unwrap_err();
        assert!(err.contains("export-credentials"), "{err}");
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
