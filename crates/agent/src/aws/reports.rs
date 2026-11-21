//! How an update ended, written to `reports/<installation>/` in the release
//! bucket (decision 0001): the installation role may write only under its
//! own id there, and read none. A report carries versions, the outcome and
//! the engine's error text, never data of the organisation.

use chrono::{DateTime, Utc};
use serde::Serialize;
use updater::state::{Outcome, OutcomeKind};

use super::client::{AwsError, Bucket};

/// The only report format this agent writes.
pub const FORMAT: u32 = 1;

/// One report: one object per outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report<'a> {
    pub format: u32,
    pub installation: &'a str,
    /// The agent that wrote it.
    pub agent: &'a str,
    /// `updated`, `rolled_back`, `restored`, `critical`, …, or `check`.
    pub kind: &'a str,
    /// The platform version attempted, and the one it came from.
    pub version: &'a str,
    pub from: &'a str,
    pub at: DateTime<Utc>,
    /// What went wrong, in the engine's words.
    pub detail: &'a str,
}

/// The name an outcome has in a report (the state file's).
fn kind_name(kind: OutcomeKind) -> &'static str {
    match kind {
        OutcomeKind::Updated => "updated",
        OutcomeKind::RolledBack => "rolled_back",
        OutcomeKind::Restored => "restored",
        OutcomeKind::Critical => "critical",
        OutcomeKind::Postponed => "postponed",
        OutcomeKind::Failed => "failed",
    }
}

impl<'a> Report<'a> {
    pub fn of(installation: &'a str, outcome: &'a Outcome) -> Self {
        Self {
            format: FORMAT,
            installation,
            agent: env!("CARGO_PKG_VERSION"),
            kind: kind_name(outcome.kind),
            version: &outcome.version,
            from: &outcome.from,
            at: outcome.at,
            detail: &outcome.detail,
        }
    }

    /// Its key: `reports/<installation>/<time>-<kind>-<version>.json`.
    pub fn key(&self) -> String {
        format!(
            "reports/{}/{}-{}-{}.json",
            self.installation,
            self.at.format("%Y%m%dT%H%M%SZ"),
            self.kind,
            self.version
        )
    }

    /// Write it; returns its key.
    pub fn write(&self, bucket: &Bucket) -> Result<String, AwsError> {
        let key = self.key();
        let body = serde_json::to_vec_pretty(self).map_err(|e| AwsError::Failed(e.to_string()))?;
        bucket.put(&key, body, "application/json")?;
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::super::client::tests::{bucket, FakeAws};
    use super::*;

    #[test]
    fn an_outcome_is_written_under_the_installation() {
        let aws = FakeAws::default();
        aws.answer(
            "PUT /reports/inst-0001/20261121T030000Z-rolled_back-1.4.0.json",
            200,
            b"",
        );
        let outcome = Outcome {
            kind: OutcomeKind::RolledBack,
            version: "1.4.0".into(),
            from: "1.3.0".into(),
            detail: "the new version did not answer".into(),
            at: Utc.with_ymd_and_hms(2026, 11, 21, 3, 0, 0).unwrap(),
        };
        let key = Report::of("inst-0001", &outcome)
            .write(&bucket(&aws))
            .unwrap();
        assert_eq!(
            key,
            "reports/inst-0001/20261121T030000Z-rolled_back-1.4.0.json"
        );
        let body: serde_json::Value = serde_json::from_slice(&aws.sent.borrow()[0].body).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "format": 1,
                "installation": "inst-0001",
                "agent": env!("CARGO_PKG_VERSION"),
                "kind": "rolled_back",
                "version": "1.4.0",
                "from": "1.3.0",
                "at": "2026-11-21T03:00:00Z",
                "detail": "the new version did not answer",
            })
        );
    }
}
