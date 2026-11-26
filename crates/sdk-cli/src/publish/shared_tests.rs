use super::*;

const HOST: &str = "1.dkr.ecr.us-east-1.amazonaws.com/quality-data";

fn image(module: &str, name: &str, c: char) -> String {
    format!(
        "{HOST}/{module}/{name}@sha256:{}",
        std::iter::repeat_n(c, 64).collect::<String>()
    )
}

fn manifests(worker_requires: u32) -> BTreeMap<String, Manifest> {
    let engine = "name: engine-trino\nversion: 0.1.0\nprovides:\n  capabilities:\n    \
                  - { name: trino-cluster, version: 484, protocols: [trino-http] }\n";
    let worker = format!(
        "name: query-trino\nversion: 0.1.0\nrequires:\n  capabilities:\n    \
         - {{ name: trino-cluster, version: {worker_requires}, protocols: [trino-http] }}\n"
    );
    BTreeMap::from([
        ("engine-trino".into(), Manifest::parse(engine).unwrap()),
        ("query-trino".into(), Manifest::parse(&worker).unwrap()),
    ])
}

fn images(engine: char, worker: char) -> BTreeMap<String, BTreeMap<String, String>> {
    BTreeMap::from([
        (
            "engine-trino".into(),
            BTreeMap::from([
                ("trino".into(), image("engine-trino", "trino", engine)),
                ("runner".into(), image("engine-trino", "runner", 'e')),
            ]),
        ),
        (
            "query-trino".into(),
            BTreeMap::from([("trino".into(), image("query-trino", "trino", worker))]),
        ),
    ])
}

#[test]
fn the_same_digest_in_each_module_s_repository_is_published() {
    check("1.5.0", &images('a', 'a'), &manifests(484)).unwrap();
}

#[test]
fn a_different_digest_of_the_shared_image_is_refused() {
    let err = check("1.5.0", &images('a', 'b'), &manifests(484)).unwrap_err();
    assert!(err.starts_with("1.5.0 cannot be published"), "{err}");
    assert!(
        err.contains("query-trino requires trino-cluster 484 of engine-trino, and mirrors trino"),
        "{err}"
    );
    assert!(
        err.contains(&"b".repeat(64)) && err.contains(&"a".repeat(64)),
        "{err}"
    );
}

#[test]
fn modules_not_bound_by_a_requirement_at_that_version_may_differ() {
    // Another version of the capability is not this provider's: the agent
    // refuses that mismatch; the images are not compared.
    check("1.5.0", &images('a', 'b'), &manifests(485)).unwrap();
    // An image only one of them names is not shared.
    let mut only = images('a', 'a');
    only.get_mut("query-trino")
        .unwrap()
        .insert("runner-extra".into(), image("query-trino", "x", 'f'));
    check("1.5.0", &only, &manifests(484)).unwrap();
    // A module without a manifest is not checked.
    let mut m = manifests(484);
    m.remove("query-trino");
    check("1.5.0", &images('a', 'b'), &m).unwrap();
}
