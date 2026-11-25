use super::*;

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
    let roles = derive("lake-rustfs", &m, COMPOSE, &env());
    assert_eq!(roles.len(), 2);
    assert_eq!(
        roles[0],
        ModuleRole {
            name: "lake".into(),
            strategy: "all".into(),
            services: vec!["rustfs".into(), "gate".into(), "admin".into()],
            images: vec![
                format!("r.example/lake/admin@sha256:{}", digest('a')),
                format!("r.example/lake/gate@sha256:{}", digest('b')),
                format!("r.example/lake/rustfs@sha256:{}", digest('a')),
            ],
        }
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
    let roles = derive("lake-rustfs", &m, COMPOSE, &env());
    let names: Vec<&str> = roles.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["lake"]);
    // A variable the .env does not set, and an image not pinned.
    let unset = env().replace("IMAGE_GATE=", "OTHER=");
    let m = manifest("    - { name: lake, services: [gate], strategy: all }\n");
    assert!(derive("lake-rustfs", &m, COMPOSE, &unset).is_empty());
    let tagged = "services:\n  gate:\n    image: r.example/lake/gate:1.0\n";
    assert!(derive("lake-rustfs", &m, tagged, &env()).is_empty());
}

#[test]
fn a_literal_pinned_image_is_taken_as_written() {
    let image = format!("r.example/lake/gate@sha256:{}", digest('c'));
    let compose = format!("services:\n  gate:\n    image: {image}\n");
    let m = manifest("    - { name: lake, services: [gate], strategy: all }\n");
    assert_eq!(
        derive("lake-rustfs", &m, &compose, "").remove(0).images,
        [image]
    );
}

#[test]
fn no_topology_or_no_readable_compose_tells_no_role() {
    let plain = Manifest::parse("name: docs\nversion: 0.2.0\n").unwrap();
    assert!(derive("docs", &plain, COMPOSE, &env()).is_empty());
    let m = manifest("    - { name: lake, services: [rustfs], strategy: all }\n");
    assert!(derive("lake-rustfs", &m, "services: [", &env()).is_empty());
}
