//! `oidc.machine.audiences` (decision 0009, rulings 30 and 47): the core's
//! own audiences a machine client asks for, the agent that first reads
//! them, and what does not read.

use module_manifest::{Manifest, MACHINE_AUDIENCES_AGENT, MAX_MACHINE_AUDIENCES};

fn manifest(machine: &str) -> Result<Manifest, module_manifest::ManifestError> {
    Manifest::parse(&format!(
        "name: orchestrator\nversion: 0.1.0\n\
         requires:\n  capabilities:\n    - {{ name: sql-query, version: 1, protocols: [trino] }}\n\
         oidc:\n  machine:\n{machine}"
    ))
}

#[test]
fn a_machine_client_reads_its_audiences_and_needs_the_agent_that_reads_them() {
    let m =
        manifest("    capabilities: [sql-query]\n    audiences: [quality-jobs, core]\n").unwrap();
    let audiences: Vec<&String> = m.machine_audiences().collect();
    assert_eq!(audiences, ["quality-jobs", "core"]);
    assert_eq!(
        m.agent_needed().map(|v| v.to_string()).as_deref(),
        Some(MACHINE_AUDIENCES_AGENT)
    );
}

#[test]
fn without_audiences_a_manifest_needs_what_it_needed_before() {
    let m = manifest("    capabilities: [sql-query]\n").unwrap();
    assert_eq!(m.machine_audiences().count(), 0);
    assert_eq!(
        m.agent_needed().map(|v| v.to_string()).as_deref(),
        Some(module_manifest::MODULE_ADD_AGENT)
    );
}

#[test]
fn a_bad_audience_list_is_refused() {
    let many: Vec<String> = (0..=MAX_MACHINE_AUDIENCES)
        .map(|i| format!("a{i}"))
        .collect();
    for audiences in [
        "[quality-jobs, quality-jobs]".to_string(),
        "[\"\"]".to_string(),
        "[\"quality jobs\"]".to_string(),
        format!("[{}]", "a".repeat(65)),
        format!("[{}]", many.join(", ")),
    ] {
        let text = format!("    capabilities: [sql-query]\n    audiences: {audiences}\n");
        assert!(manifest(&text).is_err(), "{audiences}");
    }
}

#[test]
fn audiences_alone_need_no_capability() {
    let m = manifest("    capabilities: []\n    audiences: [core]\n").unwrap();
    let audiences: Vec<&String> = m.machine_audiences().collect();
    assert_eq!(audiences, ["core"]);
    assert!(manifest("    audiences: [core]\n").is_ok());
    assert!(manifest("    capabilities: []\n").is_err());
}
