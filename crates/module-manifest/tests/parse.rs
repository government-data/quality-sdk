//! Reading `module.yaml`: the capabilities, `health` and `data` as decision
//! 0002 writes them, their defaults, and what does not read.

use module_manifest::{Manifest, ManifestError, CHECK_TIMEOUT_SECS};

const QUERY_TRINO: &str = r"
name: query-trino
version: 1.2.0
provides:
  capabilities:
    - name: sql-query
      version: 1
      protocols: [trino-client-protocol, http-access-rules, mtls-internal]
      hold: { ttl: 15m, roles: [trino-coordinator] }
requires:
  capabilities:
    - name: lake
      version: 1
      protocols: [s3, iceberg-rest, sts]
health:
  services: [coordinator, worker]
  checks:
    - service: coordinator
      command: [trino, --execute, 'select 1']
      timeout_secs: 20
data:
  volumes:
    - name: catalog
      service: coordinator
      path: /var/lib/trino
  keys:
    - path: secrets/shared.key
    - path: cache/spill
      critical: false
";

fn malformed(text: &str) -> String {
    match Manifest::parse(text) {
        Err(ManifestError::Malformed(why)) => why,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn a_manifest_reads_its_capabilities_health_and_data() {
    let m = Manifest::parse(QUERY_TRINO).unwrap();
    assert_eq!(m.name, "query-trino");
    assert_eq!(m.version.to_string(), "1.2.0");
    let sql = &m.provides.capabilities[0];
    assert_eq!((sql.name.as_str(), sql.version), ("sql-query", 1));
    assert_eq!(sql.protocols.len(), 3);
    assert_eq!(sql.hold.as_ref().unwrap().ttl_secs(), 15 * 60);
    let lake = &m.requires.capabilities[0];
    assert!(!lake.optional && lake.hold.is_none());
    assert_eq!(m.health.services, ["coordinator", "worker"]);
    assert_eq!(m.health.checks[0].command[2], "select 1");
    assert_eq!(m.health.checks[0].timeout_secs, 20);
    assert!(m.data.volumes[0].critical);
    assert_eq!(m.data.keys.len(), 2);
    let critical = m.data.critical();
    assert_eq!(critical.keys.len(), 1);
    assert_eq!(critical.keys[0].path, "secrets/shared.key");
}

#[test]
fn a_manifest_with_name_and_version_only_has_none_of_the_rest() {
    let m = Manifest::parse("name: docs\nversion: 0.1.0\n").unwrap();
    assert!(m.provides.capabilities.is_empty() && m.requires.capabilities.is_empty());
    assert!(m.health.services.is_empty() && m.health.checks.is_empty());
    assert!(m.data.databases.is_empty() && m.data.volumes.is_empty() && m.data.keys.is_empty());
}

#[test]
fn defaults_a_check_timeout_critical_data_and_required_capabilities() {
    let m = Manifest::parse(
        r"
name: quality-data
version: 2.0.0
requires:
  capabilities:
    - { name: lake, version: 1, protocols: [s3], optional: true }
    - { name: mail, version: 2, protocols: [smtp] }
health:
  checks: [{ service: api, command: [quality-data, health] }]
data:
  databases: [{ name: quality }]
",
    )
    .unwrap();
    assert!(m.requires.capabilities[0].optional);
    assert!(!m.requires.capabilities[1].optional);
    assert_eq!(m.health.checks[0].timeout_secs, CHECK_TIMEOUT_SECS);
    assert!(m.data.databases[0].critical);
}

#[test]
fn properties_and_the_sections_later_steps_define_are_accepted() {
    let m = Manifest::parse(
        r"
name: lake-rustfs
version: 1.0.0
images: { rustfs: r/lake@sha256:00 }
route: { path: /lake }
auth: oidc
menu: { label: Lake }
consumes: { apis: [core] }
permissions: { actions: [] }
config: { keys: [] }
topology: { roles: [lake] }
requirements: { core: '>=2' }
provides:
  apis: []
  events: []
  capabilities:
    - name: lake
      version: 1
      protocols: [s3, iceberg-rest, sts, mtls-gate, lake-admin/1]
      properties:
        credential-vending: unsupported
        drop-purge: unsupported
        endpoints: { s3: https://lake.internal }
",
    )
    .unwrap();
    let lake = &m.provides.capabilities[0];
    assert_eq!(lake.properties["credential-vending"], "unsupported");
    assert_eq!(lake.properties["endpoints"]["s3"], "https://lake.internal");
}

#[test]
fn a_key_it_does_not_know_or_a_wrong_type_is_malformed() {
    malformed("name: docs\nversion: 0.1.0\nlicence: x\n");
    malformed("name: docs\nversion: 0.1.0\nhealth: { probes: [] }\n");
    malformed("name: docs\nversion: one\n");
    malformed("name: docs\n");
    malformed("name: docs\nversion: 0.1.0\nrequires: { capabilities: [{ name: lake }] }\n");
}
