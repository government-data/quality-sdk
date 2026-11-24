//! `data.volumes[].backup` (decision 0002): `dump`, the default, is copied
//! into the backup; `replica` is backed up by a replica in sync instead.

use module_manifest::{Backup, Manifest, ManifestError};

fn with_volumes(volumes: &str) -> String {
    format!("name: lake\nversion: 0.1.0\ndata:\n  volumes:\n{volumes}")
}

fn invalid(text: &str) -> String {
    match Manifest::parse(text) {
        Err(ManifestError::Invalid(why)) => why,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn a_volume_is_dumped_unless_it_says_otherwise() {
    let m = Manifest::parse(&with_volumes(
        "    - { name: catalog, service: s, path: /var/lib/s }\n",
    ))
    .unwrap();
    assert_eq!(m.data.volumes[0].backup, Backup::Dump);
    assert_eq!(m.data.dumped().volumes.len(), 1);
    assert_eq!(m.data.replicated().count(), 0);
}

#[test]
fn a_replicated_volume_is_not_dumped() {
    let m = Manifest::parse(&with_volumes(concat!(
        "    - { name: rustfs-data, service: rustfs, path: /data, backup: replica }\n",
        "    - { name: config, service: rustfs, path: /etc/rustfs, backup: dump }\n",
        "    - { name: cache, service: rustfs, path: /cache, critical: false }\n",
    )))
    .unwrap();
    let replicated: Vec<_> = m.data.replicated().map(|v| v.name.as_str()).collect();
    assert_eq!(replicated, ["rustfs-data"]);
    let dumped: Vec<_> = m
        .data
        .dumped()
        .volumes
        .iter()
        .map(|v| v.name.clone())
        .collect();
    assert_eq!(dumped, ["config"]);
    // Still critical: kept on remove, as every critical piece.
    assert_eq!(m.data.critical().volumes.len(), 2);
}

#[test]
fn a_replica_backs_up_only_a_critical_volume() {
    let why = invalid(&with_volumes(
        "    - { name: cache, service: s, path: /c, critical: false, backup: replica }\n",
    ));
    assert!(
        why.contains("backup: replica needs critical: true"),
        "{why}"
    );
}

#[test]
fn another_way_of_backing_up_does_not_read() {
    let text = with_volumes("    - { name: v, service: s, path: /v, backup: snapshot }\n");
    assert!(matches!(
        Manifest::parse(&text),
        Err(ManifestError::Malformed(_))
    ));
}
