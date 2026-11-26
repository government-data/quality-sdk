use std::collections::BTreeMap;

use super::*;

/// A package of files by path.
#[derive(Default)]
pub(super) struct Fake(pub BTreeMap<String, Vec<u8>>);

impl Package for Fake {
    fn files_under(&self, path: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        let dir = format!("{path}/");
        let found: Vec<_> = self
            .0
            .iter()
            .filter(|(p, _)| p.as_str() == path || p.starts_with(&dir) || path.is_empty())
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect();
        if found.is_empty() {
            return Err(format!("{path}: not found"));
        }
        Ok(found)
    }
}

const COMPOSE: &str = r#"
x-common: &common
  restart: unless-stopped
services:
  rustfs:
    <<: *common
    image: ${IMAGE_RUSTFS:?set IMAGE_RUSTFS}
  gate:
    image: "${IMAGE_GATE}"
  admin:
    image: ${IMAGE_ADMIN:-ignored}
  tools:
    build: .
"#;

fn digest(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

fn env() -> String {
    format!(
        "# images\nIMAGE_RUSTFS=r.example/lake/rustfs@sha256:{a}\n\
         IMAGE_GATE=r.example/lake/gate@sha256:{b}\nIMAGE_ADMIN=r.example/lake/admin@sha256:{a}\n\
         PORT=9000\n",
        a = digest('a'),
        b = digest('b'),
    )
}

fn manifest(roles: &str) -> Manifest {
    Manifest::parse(&format!(
        "name: lake-rustfs\nversion: 0.2.1\ntopology:\n  roles:\n{roles}"
    ))
    .unwrap()
}

#[test]
fn a_role_gets_the_images_of_its_services() {
    let m = manifest(
        "    - { name: lake, services: [rustfs, gate, admin], strategy: all }\n\
         \x20   - { name: lake-gate, services: [gate], strategy: one }\n",
    );
    let roles = derive("lake-rustfs", &m, COMPOSE, &env(), &Fake::default());
    assert_eq!(roles.len(), 2);
    assert_eq!(
        (roles[0].name.as_str(), roles[0].strategy.as_str()),
        ("lake", "all")
    );
    assert_eq!(roles[0].services, ["rustfs", "gate", "admin"]);
    assert_eq!(
        roles[0].images,
        [
            format!("r.example/lake/admin@sha256:{}", digest('a')),
            format!("r.example/lake/gate@sha256:{}", digest('b')),
            format!("r.example/lake/rustfs@sha256:{}", digest('a')),
        ]
    );
    assert_eq!(roles[1].strategy, "one");
    assert_eq!(
        roles[1].images,
        [format!("r.example/lake/gate@sha256:{}", digest('b'))]
    );
}

#[test]
fn a_role_whose_images_cannot_all_be_told_is_left_out() {
    let m = manifest(
        "    - { name: missing, services: [rustfs, nowhere], strategy: all }\n\
         \x20   - { name: built, services: [tools], strategy: all }\n\
         \x20   - { name: lake, services: [rustfs], strategy: all }\n",
    );
    let roles = derive("lake-rustfs", &m, COMPOSE, &env(), &Fake::default());
    let names: Vec<&str> = roles.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["lake"]);
    // A variable the .env does not set, and an image not pinned.
    let unset = env().replace("IMAGE_GATE=", "OTHER=");
    let m = manifest("    - { name: lake, services: [gate], strategy: all }\n");
    assert!(derive("lake-rustfs", &m, COMPOSE, &unset, &Fake::default()).is_empty());
    let tagged = "services:\n  gate:\n    image: r.example/lake/gate:1.0\n";
    assert!(derive("lake-rustfs", &m, tagged, &env(), &Fake::default()).is_empty());
}

#[test]
fn a_literal_pinned_image_is_taken_as_written() {
    let image = format!("r.example/lake/gate@sha256:{}", digest('c'));
    let compose = format!("services:\n  gate:\n    image: {image}\n");
    let m = manifest("    - { name: lake, services: [gate], strategy: all }\n");
    assert_eq!(
        derive("lake-rustfs", &m, &compose, "", &Fake::default())
            .remove(0)
            .images,
        [image]
    );
}

#[test]
fn no_topology_or_no_readable_compose_tells_no_role() {
    let plain = Manifest::parse("name: docs\nversion: 0.2.0\n").unwrap();
    assert!(derive("docs", &plain, COMPOSE, &env(), &Fake::default()).is_empty());
    let m = manifest("    - { name: lake, services: [rustfs], strategy: all }\n");
    assert!(derive("lake-rustfs", &m, "services: [", &env(), &Fake::default()).is_empty());
}

const MOUNTS: &str = r#"
services:
  rustfs:
    image: ${IMAGE_RUSTFS}
    volumes:
      - rustfs-data:/data
      - ./compose/rustfs-tls.sh:/etc/qd/tls.sh:ro
      - ${QD_SECRETS_DIR:?}/key:/run/key:ro
      - /etc/hosts:/etc/hosts:ro
      - /anonymous
  gate:
    image: ${IMAGE_GATE}
    volumes:
      - type: bind
        source: ./gate
        target: /etc/gate
      - type: volume
        source: gate-cache
        target: /cache
volumes:
  rustfs-data: {}
"#;

fn package() -> Fake {
    Fake(BTreeMap::from([
        ("compose/rustfs-tls.sh".into(), b"#!/bin/sh\n".to_vec()),
        ("gate/entrypoint.sh".into(), b"exec gate\n".to_vec()),
        ("gate/conf/gate.yml".into(), b"port: 1\n".to_vec()),
        ("module.yaml".into(), b"not mounted\n".to_vec()),
    ]))
}

fn paths(role: &ModuleRole) -> Vec<&str> {
    role.files.iter().map(|f| f.path.as_str()).collect()
}

#[test]
fn a_role_gets_compose_yml_and_the_package_files_its_services_mount() {
    let m = manifest(
        "    - { name: lake, services: [rustfs, gate], strategy: all }\n\
         \x20   - { name: lake-tls, services: [rustfs], strategy: one }\n",
    );
    let roles = derive("lake-rustfs", &m, MOUNTS, &env(), &package());
    assert_eq!(
        paths(&roles[0]),
        [
            "compose.yml",
            "compose/rustfs-tls.sh",
            "gate/conf/gate.yml",
            "gate/entrypoint.sh"
        ]
    );
    assert_eq!(paths(&roles[1]), ["compose.yml", "compose/rustfs-tls.sh"]);
    let tls = &roles[1].files[1];
    assert_eq!(tls.content_base64, "IyEvYmluL3NoCg==");
    // `printf '#!/bin/sh\n' | sha256sum`
    assert_eq!(
        tls.sha256,
        "a8076d3d28d21e02012b20eaf7dbf75409a6277134439025f282e368e3305abf"
    );
    let compose = &roles[1].files[0];
    assert_eq!(compose.size, MOUNTS.len());
    let sent = serde_json::to_value(compose).unwrap();
    assert!(sent.get("size").is_none(), "{sent}");
    assert_eq!(sent.as_object().unwrap().len(), 3);
}

#[test]
fn a_mount_out_of_the_package_or_a_missing_file_leaves_the_role_out() {
    let m = manifest(
        "    - { name: lake, services: [rustfs], strategy: all }\n\
         \x20   - { name: lake-gate, services: [gate], strategy: all }\n",
    );
    let escaping = MOUNTS.replace("./compose/rustfs-tls.sh", "../outside.sh");
    let names: Vec<String> = derive("lake-rustfs", &m, &escaping, &env(), &package())
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(names, ["lake-gate"]);
    let mut missing = package();
    missing.0.remove("compose/rustfs-tls.sh");
    let names: Vec<String> = derive("lake-rustfs", &m, MOUNTS, &env(), &missing)
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(names, ["lake-gate"]);
}

#[test]
fn above_256_kib_the_module_tells_no_role() {
    let m = manifest(
        "    - { name: lake, services: [rustfs], strategy: all }\n\
         \x20   - { name: lake-gate, services: [gate], strategy: all }\n",
    );
    let mut big = package();
    let fits = MAX_MODULE_BYTES - MOUNTS.len() - "exec gate\n".len() - "port: 1\n".len();
    big.0
        .insert("compose/rustfs-tls.sh".into(), vec![b'x'; fits]);
    assert_eq!(derive("lake-rustfs", &m, MOUNTS, &env(), &big).len(), 2);
    big.0
        .insert("compose/rustfs-tls.sh".into(), vec![b'x'; fits + 1]);
    assert!(derive("lake-rustfs", &m, MOUNTS, &env(), &big).is_empty());
}

#[test]
fn a_role_carries_the_image_values_compose_reads() {
    let m = manifest("    - { name: lake-gate, services: [gate], strategy: one }\n");
    let role = derive("lake-rustfs", &m, COMPOSE, &env(), &Fake::default()).remove(0);
    let names: Vec<&str> = role.env.keys().map(String::as_str).collect();
    assert_eq!(names, ["IMAGE_ADMIN", "IMAGE_GATE", "IMAGE_RUSTFS"]);
    assert_eq!(
        role.env["IMAGE_GATE"],
        format!("r.example/lake/gate@sha256:{}", digest('b'))
    );
    assert!(!role.env.contains_key("PORT"), "only IMAGE_* values");
    // One the .env does not set is left out; one not pinned leaves the role out.
    let unset = env().replace("IMAGE_ADMIN=", "OTHER=");
    let role = derive("lake-rustfs", &m, COMPOSE, &unset, &Fake::default()).remove(0);
    assert!(!role.env.contains_key("IMAGE_ADMIN"));
    let tagged = env().replace(
        &format!("IMAGE_ADMIN=r.example/lake/admin@sha256:{}", digest('a')),
        "IMAGE_ADMIN=r.example/lake/admin:1",
    );
    assert!(derive("lake-rustfs", &m, COMPOSE, &tagged, &Fake::default()).is_empty());
}
