//! Whether installed modules meet a module's requirements, and what a
//! removal would leave without a provider (decision 0002 step 4).

use module_manifest::{lost_on_removal, unmet, Loss, Manifest, Why};

fn module(name: &str, provides: &str, requires: &str) -> Manifest {
    Manifest::parse(&format!(
        "name: {name}\nversion: 1.0.0\nprovides: {{ capabilities: [{provides}] }}\n\
         requires: {{ capabilities: [{requires}] }}\n"
    ))
    .unwrap()
}

const LAKE: &str = "{ name: lake, version: 1, protocols: [s3, iceberg-rest, sts] }";

fn lake_rustfs() -> Manifest {
    module("lake-rustfs", LAKE, "")
}

fn quality_data(requirement: &str) -> Manifest {
    module("quality-data", "", requirement)
}

#[test]
fn a_requirement_is_met_by_name_version_and_every_protocol() {
    let qd = quality_data("{ name: lake, version: 1, protocols: [s3, sts] }");
    assert!(unmet(&qd, &[lake_rustfs(), qd.clone()]).is_empty());
}

#[test]
fn a_missing_protocol_is_not_met() {
    let qd = quality_data("{ name: lake, version: 1, protocols: [s3, mtls-gate] }");
    let u = unmet(&qd, &[lake_rustfs()]);
    assert_eq!(u.len(), 1);
    assert_eq!(u[0].why, Why::MissingProtocols(vec!["mtls-gate".into()]));
    assert!(!u[0].optional);
}

#[test]
fn a_missing_version_is_not_met() {
    let qd = quality_data("{ name: lake, version: 2, protocols: [s3] }");
    let u = unmet(&qd, &[lake_rustfs()]);
    assert_eq!(u[0].why, Why::OtherVersions(vec![1]));
    assert_eq!((u[0].capability.as_str(), u[0].version), ("lake", 2));
    assert_eq!(unmet(&qd, &[])[0].why, Why::NoProvider);
}

#[test]
fn a_module_does_not_meet_its_own_requirement() {
    let both = module(
        "lake-rustfs",
        LAKE,
        "{ name: lake, version: 1, protocols: [s3] }",
    );
    assert_eq!(
        unmet(&both, std::slice::from_ref(&both))[0].why,
        Why::NoProvider
    );
}

#[test]
fn an_optional_requirement_that_is_missing_is_marked_optional() {
    let orch = module(
        "orchestrator",
        "",
        "{ name: sql-query, version: 1, protocols: [trino-client-protocol], optional: true }",
    );
    let u = unmet(&orch, &[lake_rustfs()]);
    assert!(u[0].optional);
    assert_eq!(u[0].why, Why::NoProvider);
}

#[test]
fn removing_the_only_provider_is_a_loss() {
    let qd = quality_data("{ name: lake, version: 1, protocols: [s3] }");
    let installed = [qd, lake_rustfs()];
    assert_eq!(
        lost_on_removal("lake-rustfs", &installed),
        [Loss {
            module: "quality-data".into(),
            capability: "lake".into(),
            version: 1,
            optional: false,
        }]
    );
    assert!(lost_on_removal("quality-data", &installed).is_empty());
}

#[test]
fn removing_a_provider_is_allowed_while_another_provides_it() {
    let qd = quality_data("{ name: lake, version: 1, protocols: [s3] }");
    let other = module("lake-other", LAKE, "");
    assert!(lost_on_removal("lake-rustfs", &[qd, lake_rustfs(), other]).is_empty());
}

#[test]
fn another_provider_without_the_protocols_used_does_not_count() {
    let qd = quality_data("{ name: lake, version: 1, protocols: [s3, sts] }");
    let other = module(
        "lake-other",
        "{ name: lake, version: 1, protocols: [s3] }",
        "",
    );
    let lost = lost_on_removal("lake-rustfs", &[qd, lake_rustfs(), other]);
    assert_eq!(lost.len(), 1);
}

#[test]
fn an_optional_requirement_lost_is_marked_optional() {
    let obs = module(
        "observability",
        "",
        "{ name: lake, version: 1, protocols: [s3], optional: true }",
    );
    let lost = lost_on_removal("lake-rustfs", &[obs, lake_rustfs()]);
    assert!(lost[0].optional);
}

#[test]
fn a_requirement_not_met_before_is_not_a_loss() {
    let qd = quality_data("{ name: lake, version: 2, protocols: [s3] }");
    assert!(lost_on_removal("lake-rustfs", &[qd, lake_rustfs()]).is_empty());
}
