//! The registry's credential for the installation's clusters (decision
//! 0013): only the agent holds AWS credentials, so it asks ECR for an
//! authorization token (12 hours) and hands it to quality-data
//! (`PUT /updater/v1/registry`), which passes it over mTLS to the joined
//! machines of a role that needs those images. Handed at start, every 6
//! hours after a success, and again once quality-data was told the
//! installed modules anew (its API restarted, and it keeps the token in
//! memory); 10 minutes after a failure, or while quality-data does not
//! list the feature. The token is a secret: never logged, never written.

use std::cell::Cell;

use chrono::{DateTime, Duration, Utc};

use crate::host::Features;

/// How long after a success the token is handed again.
pub const EVERY: Duration = Duration::hours(6);
/// How long after a failure (or a core without the feature) it is tried
/// again.
pub const RETRY: Duration = Duration::minutes(10);
/// How long a token lasts when ECR does not say (its documented life).
pub const LIFE: Duration = Duration::hours(12);

/// A registry's host and a temporary user and password for it.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistryToken {
    pub host: String,
    pub username: String,
    pub password: String,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for RegistryToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryToken")
            .field("host", &self.host)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Where a token comes from: ECR, with the installation's credentials.
pub trait TokenSource {
    fn token(&self, now: DateTime<Utc>) -> Result<RegistryToken, String>;
}

/// Where it goes: quality-data's `updater/v1`.
pub trait TokenSink {
    fn features(&self) -> Result<Features, String>;
    fn put_registry(&self, token: &RegistryToken) -> Result<(), String>;
}

/// What a call to [`Supply::supply_if_due`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Supplied {
    NotDue,
    /// Handed; it expires then.
    Handed(DateTime<Utc>),
    /// quality-data does not list [`Features::REGISTRY_CREDENTIALS`].
    NotOffered,
    Failed(String),
}

/// When the token is handed next (`None`: now).
#[derive(Debug, Default)]
pub struct Supply {
    next: Cell<Option<DateTime<Utc>>>,
}

impl Supply {
    /// Hand it at the next call, whenever it was last handed.
    pub fn again_now(&self) {
        self.next.set(None);
    }

    /// Hand a new token when it is due.
    pub fn supply_if_due(
        &self,
        now: DateTime<Utc>,
        source: &dyn TokenSource,
        sink: &dyn TokenSink,
    ) -> Supplied {
        if self.next.get().is_some_and(|next| now < next) {
            return Supplied::NotDue;
        }
        let done = match sink.features() {
            Ok(f) if !f.has(Features::REGISTRY_CREDENTIALS) => Supplied::NotOffered,
            Ok(_) => match source.token(now) {
                Ok(t) => match sink.put_registry(&t) {
                    Ok(()) => Supplied::Handed(t.expires_at),
                    Err(e) => Supplied::Failed(e),
                },
                Err(e) => Supplied::Failed(e),
            },
            Err(e) => Supplied::Failed(e),
        };
        let wait = if matches!(done, Supplied::Handed(_)) {
            EVERY
        } else {
            RETRY
        };
        self.next.set(Some(now + wait));
        done
    }
}

impl TokenSource for crate::aws::ecr::Ecr {
    fn token(&self, now: DateTime<Utc>) -> Result<RegistryToken, String> {
        let (auth, expires_at) = self.authorization_until().map_err(|e| e.to_string())?;
        Ok(RegistryToken {
            host: auth.host,
            username: auth.user,
            password: auth.password,
            expires_at: expires_at.unwrap_or(now + LIFE),
        })
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
