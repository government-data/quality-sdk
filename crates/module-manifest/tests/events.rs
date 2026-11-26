//! `provides.events` and `consumes.events` (decision 0017): what reads,
//! the agent that first tells them, and what does not read.

use module_manifest::{ConsumedEvent, Manifest, ProvidedEvent, EVENTS_AGENT, MAX_EVENTS};

fn manifest(name: &str, events: &str) -> Result<Manifest, module_manifest::ManifestError> {
    Manifest::parse(&format!("name: {name}\nversion: 0.1.0\n{events}"))
}

const BOTH: &str = "provides:
  events:
    - topic: orchestrator.run.finished
      version: 1
      schema: events/run-finished.v1.json
consumes:
  events:
    - topic: core.iam.changed
      version: 1
";

#[test]
fn a_module_reads_the_topics_it_provides_and_consumes() {
    let m = manifest("orchestrator", BOTH).unwrap();
    assert_eq!(
        m.provides.events,
        [ProvidedEvent {
            topic: "orchestrator.run.finished".into(),
            version: 1,
            schema: "events/run-finished.v1.json".into(),
        }]
    );
    assert_eq!(
        m.consumes.events,
        [ConsumedEvent {
            topic: "core.iam.changed".into(),
            version: 1,
        }]
    );
    assert!(m.declares_events());
    assert_eq!(
        m.agent_needed().map(|v| v.to_string()).as_deref(),
        Some(EVENTS_AGENT)
    );
}

#[test]
fn a_module_without_events_declares_none_and_needs_no_agent() {
    let m = manifest("orchestrator", "").unwrap();
    assert!(!m.declares_events());
    assert_eq!(m.agent_needed(), None);
}

#[test]
fn a_module_whose_name_has_a_hyphen_names_its_topics_with_it() {
    let m = manifest(
        "bi-superset",
        "provides:\n  events:\n    - { topic: bi-superset.dashboard.saved, version: 2, schema: s.json }\n",
    )
    .unwrap();
    assert_eq!(m.provides.events[0].version, 2);
}

#[test]
fn a_module_provides_only_its_own_well_named_topics() {
    for topic in [
        "core.iam.changed",
        "other.run.finished",
        "orchestrator",
        "orchestrator.",
        "orchestrator..x",
        "orchestrator.Run",
        "orchestrator.run-finished",
        ".run",
    ] {
        let text = format!(
            "provides:\n  events:\n    - {{ topic: {topic:?}, version: 1, schema: s.json }}\n"
        );
        assert!(manifest("orchestrator", &text).is_err(), "{topic}");
    }
}

#[test]
fn a_schema_is_a_json_file_inside_the_package() {
    for schema in [
        "/etc/s.json",
        "../s.json",
        "events/../../s.json",
        "./s.json",
        "events//s.json",
        "s.yaml",
        "",
    ] {
        let text = format!(
            "provides:\n  events:\n    - {{ topic: orchestrator.run, version: 1, schema: {schema:?} }}\n"
        );
        assert!(manifest("orchestrator", &text).is_err(), "{schema}");
    }
}

#[test]
fn versions_start_at_one_and_each_topic_version_appears_once() {
    for events in [
        "consumes:\n  events:\n    - { topic: core.iam.changed, version: 0 }\n",
        "consumes:\n  events:\n    - { topic: core.iam.changed, version: 1 }\n    - { topic: core.iam.changed, version: 1 }\n",
        "provides:\n  events:\n    - { topic: orchestrator.run, version: 0, schema: s.json }\n",
        "provides:\n  events:\n    - { topic: orchestrator.run, version: 1, schema: a.json }\n    - { topic: orchestrator.run, version: 1, schema: b.json }\n",
        "consumes:\n  events:\n    - { topic: core.iam.changed, version: 1, schema: s.json }\n",
    ] {
        assert!(manifest("orchestrator", events).is_err(), "{events}");
    }
    let both = "consumes:\n  events:\n    - { topic: core.iam.changed, version: 1 }\n    - { topic: core.iam.changed, version: 2 }\n";
    assert_eq!(
        manifest("orchestrator", both)
            .unwrap()
            .consumes
            .events
            .len(),
        2
    );
}

#[test]
fn a_module_declares_at_most_so_many_topics() {
    let many = |n: usize| {
        let lines: String = (0..n)
            .map(|i| format!("    - {{ topic: core.t{i}, version: 1 }}\n"))
            .collect();
        format!("consumes:\n  events:\n{lines}")
    };
    assert!(manifest("orchestrator", &many(MAX_EVENTS)).is_ok());
    assert!(manifest("orchestrator", &many(MAX_EVENTS + 1)).is_err());
}
