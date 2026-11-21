//! What the administrator decided (quality-data's API stores it and the
//! daemon reads it from `GET /internal/orders`): when to update, and whether
//! there is an "Update now" request.

use chrono::{DateTime, Datelike, Duration, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// How this installation updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// It only tells; a person on the server starts the update.
    Manual,
    /// It tells and waits for an administrator to press "Update".
    #[default]
    Approval,
    /// By itself, inside the window.
    Automatic,
}

/// The automatic update's window, in the installation's time. If `start` is
/// after `end` it crosses midnight, and it counts the day it starts.
///
/// The time zone is the installation's own setting (decision 0003): UTC when
/// it is not given or does not parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// 1 = Monday … 7 = Sunday.
    pub days: Vec<u8>,
    /// "HH:MM"
    pub start: String,
    pub end: String,
    #[serde(default = "utc")]
    pub timezone: String,
}

fn utc() -> String {
    "UTC".into()
}

impl Default for Window {
    fn default() -> Self {
        Self {
            days: (1..=7).collect(),
            start: "02:00".into(),
            end: "05:00".into(),
            timezone: utc(),
        }
    }
}

/// The whole policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub mode: Mode,
    pub window: Window,
    /// How long to wait for what runs to finish before postponing.
    pub max_drain_secs: u64,
    /// In automatic mode, do not start if a schedule fires within this time
    /// (0: do not look).
    pub skip_if_scheduled_within_secs: u64,
    /// Paused by an administrator: it does not update by itself.
    pub paused: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            window: Window::default(),
            max_drain_secs: 30 * 60,
            skip_if_scheduled_within_secs: 15 * 60,
            paused: false,
        }
    }
}

/// An "Update now" pressed on the web.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
}

/// What `GET /internal/orders` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Orders {
    pub policy: Policy,
    pub request: Option<Request>,
}

impl Window {
    fn zone(&self) -> Tz {
        self.timezone.parse().unwrap_or(chrono_tz::UTC)
    }

    fn hours(&self) -> Option<(NaiveTime, NaiveTime)> {
        let h = |s: &str| NaiveTime::parse_from_str(s.trim(), "%H:%M").ok();
        Some((h(&self.start)?, h(&self.end)?))
    }

    fn includes(&self, day: chrono::Weekday) -> bool {
        let n = u8::try_from(day.number_from_monday()).unwrap_or(0);
        self.days.contains(&n)
    }

    /// Whether `now` falls inside the window. A badly written window (hours
    /// that do not parse, or equal) never opens.
    pub fn contains(&self, now: DateTime<Utc>) -> bool {
        self.end_of(now).is_some()
    }

    /// When the window `now` is in ends (or `None` if it is in none).
    pub fn end_of(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let (start, end) = self.hours()?;
        if start == end {
            return None;
        }
        let zone = self.zone();
        let local = now.with_timezone(&zone);
        let today = local.date_naive();
        let t = local.time();
        let ends_on = if start < end {
            (self.includes(local.weekday()) && start <= t && t < end).then_some(today)?
        } else if t >= start && self.includes(local.weekday()) {
            today.succ_opt()?
        } else if t < end && self.includes(local.weekday().pred()) {
            today
        } else {
            return None;
        };
        let end_local = ends_on.and_time(end);
        // A clock change can make that hour not exist, or exist twice.
        let end = zone
            .from_local_datetime(&end_local)
            .earliest()
            .or_else(|| {
                zone.from_local_datetime(&(end_local + Duration::hours(1)))
                    .earliest()
            })?;
        Some(end.with_timezone(&Utc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bogota(s: &str) -> DateTime<Utc> {
        // Bogota is UTC-5 all year.
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M")
            .unwrap()
            .and_utc()
            + Duration::hours(5)
    }

    fn window(days: &[u8], start: &str, end: &str) -> Window {
        Window {
            days: days.to_vec(),
            start: start.into(),
            end: end.into(),
            timezone: "America/Bogota".into(),
        }
    }

    #[test]
    fn an_early_morning_window() {
        let w = window(&[1, 2, 3, 4, 5, 6, 7], "02:00", "05:00");
        // 2026-10-19 is a Monday.
        assert!(!w.contains(bogota("2026-10-19 01:59")));
        assert!(w.contains(bogota("2026-10-19 02:00")));
        assert!(w.contains(bogota("2026-10-19 04:59")));
        assert!(!w.contains(bogota("2026-10-19 05:00")));
        assert_eq!(
            w.end_of(bogota("2026-10-19 03:00")),
            Some(bogota("2026-10-19 05:00"))
        );
    }

    #[test]
    fn only_the_chosen_days() {
        // Only Saturdays and Sundays.
        let w = window(&[6, 7], "02:00", "05:00");
        assert!(!w.contains(bogota("2026-10-23 03:00")), "Friday");
        assert!(w.contains(bogota("2026-10-24 03:00")), "Saturday");
        assert!(w.contains(bogota("2026-10-25 03:00")), "Sunday");
    }

    #[test]
    fn a_window_across_midnight_counts_the_day_it_starts() {
        // Saturday from 22:00 to Sunday 02:00.
        let w = window(&[6], "22:00", "02:00");
        assert!(!w.contains(bogota("2026-10-24 21:59")));
        assert!(w.contains(bogota("2026-10-24 23:30")), "Saturday night");
        assert!(
            w.contains(bogota("2026-10-25 01:30")),
            "Sunday early morning"
        );
        assert!(
            !w.contains(bogota("2026-10-25 23:30")),
            "Sunday does not start"
        );
        assert!(!w.contains(bogota("2026-10-24 01:30")), "comes from Friday");
        assert_eq!(
            w.end_of(bogota("2026-10-24 23:30")),
            Some(bogota("2026-10-25 02:00"))
        );
        assert_eq!(
            w.end_of(bogota("2026-10-25 01:30")),
            Some(bogota("2026-10-25 02:00"))
        );
    }

    #[test]
    fn a_clock_change_does_not_break_the_window() {
        // In Madrid, on 2026-10-25 at 03:00 it is 02:00 again.
        let w = Window {
            days: (1..=7).collect(),
            start: "01:00".into(),
            end: "04:00".into(),
            timezone: "Europe/Madrid".into(),
        };
        // 02:30 summer time (00:30 UTC).
        let now = DateTime::parse_from_rfc3339("2026-10-25T00:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(w.contains(now));
        // That day's 04:00 is already winter time: 03:00 UTC.
        assert_eq!(
            w.end_of(now),
            Some(
                DateTime::parse_from_rfc3339("2026-10-25T03:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }

    #[test]
    fn a_badly_written_window_never_opens() {
        for w in [
            window(&[1, 2, 3, 4, 5, 6, 7], "2am", "05:00"),
            window(&[1, 2, 3, 4, 5, 6, 7], "03:00", "03:00"),
            window(&[], "02:00", "05:00"),
        ] {
            assert!(!w.contains(bogota("2026-10-19 03:00")), "{w:?}");
        }
    }

    #[test]
    fn without_a_time_zone_the_window_is_in_utc() {
        assert_eq!(Window::default().timezone, "UTC");
        let utc = |s: &str| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M")
                .unwrap()
                .and_utc()
        };
        let given: Window = serde_json::from_value(serde_json::json!({
            "days": [1, 2, 3, 4, 5, 6, 7], "start": "02:00", "end": "05:00"
        }))
        .unwrap();
        let unknown = Window {
            timezone: "Nowhere/Atlantis".into(),
            ..Window::default()
        };
        for w in [Window::default(), given, unknown] {
            assert!(!w.contains(utc("2026-10-19 01:59")), "{w:?}");
            assert!(w.contains(utc("2026-10-19 02:00")), "{w:?}");
            assert!(!w.contains(utc("2026-10-19 05:00")), "{w:?}");
        }
    }

    #[test]
    fn without_orders_from_the_api_approval_is_expected() {
        let o: Orders = serde_json::from_str("{}").unwrap();
        assert_eq!(o.policy.mode, Mode::Approval);
        assert_eq!(o.request, None);
        let o: Orders = serde_json::from_value(serde_json::json!({
            "policy": { "mode": "automatic", "paused": true },
            "request": { "id": "p1" }
        }))
        .unwrap();
        assert_eq!(o.policy.mode, Mode::Automatic);
        assert!(o.policy.paused);
        assert_eq!(o.policy.max_drain_secs, 30 * 60);
        assert_eq!(o.request, Some(Request { id: "p1".into() }));
    }
}
