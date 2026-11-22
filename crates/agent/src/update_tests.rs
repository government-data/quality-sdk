//! `update --wait`: when a request ended, read from the state file.

use chrono::{TimeZone, Utc};
use updater::state::{Modules, Target};

use super::*;

fn target() -> Target {
    Target {
        version: "1.3.0".into(),
        modules: Modules::new(),
        packages: Vec::new(),
        notes: String::new(),
    }
}

fn outcome(kind: OutcomeKind, minute: u32) -> Outcome {
    Outcome {
        kind,
        version: "1.3.0".into(),
        from: "1.2.0".into(),
        detail: String::new(),
        at: Utc.with_ymd_and_hms(2026, 11, 22, 10, minute, 0).unwrap(),
    }
}

fn state(phase: Phase, handled: Option<&str>, last: Option<Outcome>) -> State {
    let mut s = State::new("1.2.0");
    s.phase = phase;
    s.handled_request = handled.map(str::to_string);
    s.last = last;
    s
}

#[test]
fn a_request_not_handled_yet_or_still_running_has_not_ended() {
    assert_eq!(ended(&state(Phase::Idle, None, None), "r-2", None), None);
    let other = state(Phase::Idle, Some("r-1"), None);
    assert_eq!(ended(&other, "r-2", None), None, "another request");
    let draining = Phase::Draining {
        target: target(),
        since: Utc::now(),
        request: Some("r-2".into()),
    };
    assert_eq!(
        ended(&state(draining, Some("r-2"), None), "r-2", None),
        None
    );
}

#[test]
fn handled_with_no_new_attempt_there_was_nothing_newer() {
    let before = outcome(OutcomeKind::Updated, 1);
    let s = state(Phase::Idle, Some("r-2"), Some(before.clone()));
    assert_eq!(ended(&s, "r-2", Some(&before)), Some(Ended::NothingNewer));
    assert_eq!(
        ended(&state(Phase::Idle, Some("r-2"), None), "r-2", None),
        Some(Ended::NothingNewer)
    );
    assert!(Ended::NothingNewer.ok());
}

#[test]
fn a_new_attempt_is_how_it_ended() {
    let before = outcome(OutcomeKind::Updated, 1);
    let updated = outcome(OutcomeKind::Updated, 9);
    let s = state(Phase::Idle, Some("r-2"), Some(updated.clone()));
    let end = ended(&s, "r-2", Some(&before)).unwrap();
    assert_eq!(end, Ended::Attempt(updated));
    assert!(end.ok());
    assert_eq!(end.describe(), "updated 1.3.0 (from 1.2.0)");

    let mut rolled = outcome(OutcomeKind::RolledBack, 9);
    rolled.detail = "the new version did not answer".into();
    let s = state(Phase::Idle, Some("r-2"), Some(rolled));
    let end = ended(&s, "r-2", Some(&before)).unwrap();
    assert!(!end.ok());
    assert_eq!(
        end.describe(),
        "rolled_back 1.3.0 (from 1.2.0): the new version did not answer"
    );
}

#[test]
fn wait_reads_the_state_file_until_it_ended_or_the_limit() {
    let dir = std::env::temp_dir().join(format!("agent-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("state.json");
    state(Phase::Prepared { target: target() }, None, None)
        .save(&file)
        .unwrap();
    let every = (Duration::from_secs(5), Duration::from_secs(60));

    // Handled on the third read.
    let mut reads = 0;
    let mut sleep = |_: Duration| {
        reads += 1;
        if reads == 2 {
            let done = state(
                Phase::Idle,
                Some("r-2"),
                Some(outcome(OutcomeKind::Updated, 9)),
            );
            done.save(&file).unwrap();
        }
    };
    let end = wait(&file, "r-2", None, every, &mut sleep).unwrap();
    assert!(end.ok());
    assert_eq!(reads, 2);

    // Never handled: it gives up after the limit, saying so.
    let mut slept = Duration::ZERO;
    let err = wait(&file, "r-3", None, every, &mut |d| slept += d).unwrap_err();
    assert!(err.contains("r-3"), "{err}");
    assert_eq!(slept, Duration::from_secs(60));
    std::fs::remove_dir_all(&dir).unwrap();
}
