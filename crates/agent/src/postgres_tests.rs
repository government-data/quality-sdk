//! The PostgreSQL adapter with a fake runner: nothing runs `docker`; what
//! the commands do was verified by `tools/verify/postgres-backup.sh`.

use std::os::unix::fs::PermissionsExt;
use std::rc::Rc;

use super::*;

const HELPER: &str = "r/tools/busybox@sha256:b0b";
use crate::command::fake::Fake;
use crate::host::Database;
use crate::paths::Paths;

fn root(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-postgres-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fake the test keeps a handle on.
struct Shared(Rc<Fake>);
impl Runner for Shared {
    fn run(&self, cmd: &Cmd) -> Result<String, String> {
        self.0.run(cmd)
    }
}

/// A server with `quality` and `orchestrator`, whose dumps list `entries`.
fn server(entries: &str) -> Fake {
    Fake::default()
        .answer("psql", Ok("orchestrator\nquality\n"))
        .answer("pg_restore --list", Ok(entries))
}

fn postgres(root: &Path, fake: Fake) -> (Postgres, Rc<Fake>) {
    let fake = Rc::new(fake);
    let mut p = Postgres::new(&Paths::under(root), HELPER);
    p.runner = Box::new(Shared(fake.clone()));
    (p, fake)
}

/// What ran, after `docker compose … --env-file <.env>`.
fn ran(fake: &Fake) -> Vec<String> {
    fake.lines()
        .iter()
        .map(|l| l.split(".env ").nth(1).unwrap_or(l).to_string())
        .collect()
}

const LISTED: &str = ";\n; Archive created\n;\n215; 1259 16390 TABLE public items qd\n";

#[test]
fn every_database_is_dumped_in_its_container_and_read_back_whole() {
    let r = root("back-up");
    let (p, fake) = postgres(&r, server(LISTED));
    let dir = p.back_up().unwrap();
    assert!(dir.starts_with(r.join("var/lib/quality-data/backups")));
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("databases")).unwrap(),
        "orchestrator\nquality\n"
    );
    let lines = ran(&fake);
    let dump = r#"exec -T postgres sh -c pg_dump -U "$POSTGRES_USER" -d "$1" --format=custom sh"#;
    assert_eq!(
        lines[1],
        r#"exec -T postgres sh -c pg_dumpall -U "$POSTGRES_USER" --globals-only sh"#
    );
    assert_eq!(lines[2], format!("{dump} orchestrator"));
    assert_eq!(lines[3], "exec -T postgres sh -c pg_restore --list sh");
    assert_eq!(
        lines[4],
        "exec -T postgres sh -c pg_restore --file=/dev/null sh"
    );
    assert_eq!(lines[5], format!("{dump} quality"));
    let cmds = fake.ran.borrow();
    assert_eq!(
        cmds[2].stdout.as_deref(),
        Some(dir.join("orchestrator.dump").as_path())
    );
    assert_eq!(
        cmds[3].stdin.as_deref(),
        Some(dir.join("orchestrator.dump").as_path())
    );
    assert!(cmds[0].line().contains("--project-name quality-data"));
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn a_dump_that_does_not_read_back_or_holds_nothing_is_no_backup() {
    let r = root("bad-dump");
    let (p, _) = postgres(&r, server(";\n; empty\n"));
    assert!(p.back_up().unwrap_err().contains("holds nothing"));
    let cut = server(LISTED).answer(
        "--file=/dev/null",
        Err("could not read from input file: end of file"),
    );
    let (p, _) = postgres(&r, cut);
    let err = p.back_up().unwrap_err();
    assert!(
        err.contains("does not read back whole") && err.contains("end of file"),
        "{err}"
    );
    let (p, _) = postgres(&r, Fake::default().answer("psql", Ok("")));
    assert!(p.back_up().unwrap_err().contains("no database"));
    let (p, _) = postgres(&r, Fake::default().answer("psql", Ok("x; drop\n")));
    assert!(p.back_up().is_err());
    // None of them left a whole backup.
    let whole = std::fs::read_dir(r.join("var/lib/quality-data/backups"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().join("databases").exists())
        .count();
    assert_eq!(whole, 0);
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn only_the_newest_backups_are_kept() {
    let r = root("keep");
    let backups = r.join("var/lib/quality-data/backups");
    for day in 1..=12 {
        std::fs::create_dir_all(backups.join(format!("202611{day:02}T000000000Z"))).unwrap();
    }
    std::fs::create_dir_all(backups.join("by-hand")).unwrap();
    let (p, _) = postgres(&r, server(LISTED));
    p.back_up().unwrap();
    let mut left: Vec<String> = std::fs::read_dir(&backups)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left.len(), KEEP + 1, "{left:?}");
    assert_eq!(left[0], "20261104T000000000Z");
    assert!(left.contains(&"by-hand".to_string()));
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn a_restore_brings_up_the_database_alone_and_restores_each_in_one_transaction() {
    let r = root("restore");
    let backup = r.join("var/lib/quality-data/backups/20261121T120000000Z");
    std::fs::create_dir_all(&backup).unwrap();
    assert!(postgres(&r, Fake::default())
        .0
        .restore(&backup)
        .unwrap_err()
        .contains("not a whole backup"));
    std::fs::write(backup.join("databases"), "orchestrator\nquality\n").unwrap();
    std::fs::write(backup.join("orchestrator.dump"), "d").unwrap();
    assert!(postgres(&r, Fake::default())
        .0
        .restore(&backup)
        .unwrap_err()
        .contains("quality.dump is missing"));
    std::fs::write(backup.join("quality.dump"), "d").unwrap();

    let (p, fake) = postgres(&r, Fake::default());
    p.restore(&backup).unwrap();
    let restore = r#"exec -T postgres sh -c pg_restore -U "$POSTGRES_USER" -d "$1" --clean --if-exists --single-transaction sh"#;
    assert_eq!(
        ran(&fake),
        [
            "up --detach --no-build --wait --wait-timeout 300 postgres".to_string(),
            format!("{restore} orchestrator"),
            format!("{restore} quality"),
        ]
    );
    assert_eq!(
        fake.ran.borrow()[2].stdin.as_deref(),
        Some(backup.join("quality.dump").as_path())
    );

    let (p, _) = postgres(
        &r,
        Fake::default().answer("-d \"$1\" --clean", Err("lock timeout")),
    );
    assert!(p
        .restore(&backup)
        .unwrap_err()
        .contains("restoring orchestrator: lock timeout"));
    std::fs::remove_dir_all(r).unwrap();
}
