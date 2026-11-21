//! The Docker Compose adapter under a test root, with a fake runner:
//! nothing runs `docker`.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

use updater::state::Module;

use super::*;
use crate::command::fake::Fake;
use crate::paths::Paths;

fn root(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-compose-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn compose(root: &Path, fake: Fake) -> DockerCompose {
    let mut c = DockerCompose::new(&Paths::under(root));
    c.runner = Box::new(fake);
    c
}

fn digest(c: char) -> String {
    c.to_string().repeat(64)
}

/// `quality-data` with `api` and `web`, `lake-rustfs` with `rustfs` and
/// `admin-api`, at `version`.
fn modules(version: &str, c: char) -> Modules {
    let m = |images: &[&str]| Module {
        version: version.into(),
        images: images
            .iter()
            .map(|i| ((*i).to_string(), format!("r/{i}@sha256:{}", digest(c))))
            .collect::<BTreeMap<_, _>>(),
    };
    Modules::from([
        ("quality-data".to_string(), m(&["api", "web"])),
        ("lake-rustfs".to_string(), m(&["rustfs", "admin-api"])),
    ])
}

fn order(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

#[test]
fn each_image_is_a_variable_of_its_module_s_env_and_the_rest_stays() {
    let r = root("env");
    let c = compose(&r, Fake::default());
    let env = r.join("opt/quality-data/quality-data/.env");
    std::fs::create_dir_all(env.parent().unwrap()).unwrap();
    std::fs::write(
        &env,
        "# ports\nHTTP_PORT=8080\nIMAGE_OLD=r/old@sha256:x\n\n",
    )
    .unwrap();
    c.put_version("1.4.0", &modules("1.4.0", 'a'), &[]).unwrap();
    let text = std::fs::read_to_string(&env).unwrap();
    assert_eq!(
        text,
        format!(
            "# ports\nHTTP_PORT=8080\nIMAGE_API=r/api@sha256:{a}\nIMAGE_WEB=r/web@sha256:{a}\n",
            a = digest('a')
        )
    );
    let lake = std::fs::read_to_string(r.join("opt/quality-data/lake-rustfs/.env")).unwrap();
    assert!(lake.starts_with(&format!(
        "IMAGE_ADMIN_API=r/admin-api@sha256:{}\n",
        digest('a')
    )));
    let mode = std::fs::metadata(&env).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(image_variable("query-api").unwrap(), "IMAGE_QUERY_API");
    assert!(image_variable("API").is_err());
    let mut bad = modules("1.4.0", 'a');
    bad.get_mut("quality-data")
        .unwrap()
        .images
        .insert("api".into(), "r/api@sha256:x\nEVIL=1".into());
    assert!(c.put_version("1.4.0", &bad, &[]).is_err());
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn the_record_keeps_the_order_and_going_back_takes_the_previous_order() {
    let r = root("record");
    let c = compose(&r, Fake::default());
    assert!(c.installed().unwrap_err().contains("no platform version"));
    c.put_version(
        "1.3.0",
        &modules("1.3.0", 'a'),
        &order(&["lake-rustfs", "quality-data"]),
    )
    .unwrap();
    let mut next = modules("1.4.0", 'b');
    next.remove("lake-rustfs");
    c.put_version("1.4.0", &next, &order(&["quality-data"]))
        .unwrap();
    let rec = c.read_record().unwrap().unwrap();
    assert_eq!(rec.now.order, ["quality-data"]);
    assert_eq!(rec.stopping, ["lake-rustfs"], "brought down when it starts");
    assert_eq!(c.installed().unwrap().version, "1.4.0");
    // The same step again changes nothing it replaced.
    c.put_version("1.4.0", &next, &order(&["quality-data"]))
        .unwrap();
    assert_eq!(c.read_record().unwrap().unwrap(), rec);

    // Going back: the packages are put again by name, but the version
    // starts in the order it had.
    c.put_version(
        "1.3.0",
        &modules("1.3.0", 'a'),
        &order(&["lake-rustfs", "quality-data"])
            .into_iter()
            .rev()
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let back = c.read_record().unwrap().unwrap();
    assert_eq!(back.now.order, ["lake-rustfs", "quality-data"]);
    assert!(back.stopping.is_empty());
    let installed = c.installed().unwrap();
    assert_eq!(installed.modules, modules("1.3.0", 'a'));
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn modules_without_an_order_start_by_name_and_compose_is_named_after_the_module() {
    let r = root("command");
    let c = compose(&r, Fake::default());
    c.put_version("1.3.0", &modules("1.3.0", 'a'), &[]).unwrap();
    assert_eq!(
        c.read_record().unwrap().unwrap().now.order,
        ["lake-rustfs", "quality-data"]
    );
    let qd = r.join("opt/quality-data/quality-data");
    let cmd = c.compose("quality-data", &["ps"]).unwrap();
    assert_eq!(
        cmd.line(),
        format!(
            "docker compose --project-name quality-data --project-directory {q}/current \
             --file {q}/current/compose.yml --env-file {q}/.env ps",
            q = qd.display()
        )
    );
    assert!(c.compose("../x", &["ps"]).is_err());
    std::fs::remove_dir_all(r).unwrap();
}
