//! A module's role and databases with a fake runner that keeps what each
//! command read on its standard input; what PostgreSQL does with the
//! statements was verified by `tools/verify/module-database.sh` (V2).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use super::*;
use crate::command::{Cmd, Runner};
use crate::paths::Paths;

/// Every command run, with its standard input's text, read as it ran.
#[derive(Default)]
struct Seen {
    ran: RefCell<Vec<(Cmd, String)>>,
    fail: Option<String>,
}

struct Shared(Rc<Seen>);

impl Runner for Shared {
    fn run(&self, cmd: &Cmd) -> Result<String, String> {
        let stdin = cmd
            .stdin
            .as_ref()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .unwrap_or_default();
        self.0.ran.borrow_mut().push((cmd.clone(), stdin));
        match &self.0.fail {
            Some(why) => Err(why.clone()),
            None => Ok(String::new()),
        }
    }
}

fn postgres(name: &str, fail: Option<&str>) -> (Postgres, Rc<Seen>, PathBuf) {
    let root = std::env::temp_dir().join(format!("agent-module-db-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let seen = Rc::new(Seen {
        fail: fail.map(str::to_string),
        ..Seen::default()
    });
    let mut p = Postgres::new(&Paths::under(&root), "r/tools/busybox@sha256:b0b");
    p.runner = Box::new(Shared(seen.clone()));
    (p, seen, root)
}

const PASSWORD: &str = "qs-test-database-password-not-real";

#[test]
fn the_verifier_is_postgresqls_scram_sha_256() {
    // Computed independently (Python's hashlib and hmac) for this salt.
    let salt = STANDARD.decode("W22ZaJ0SNY7soEsUEjb6gQ==").unwrap();
    assert_eq!(
        scram_verifier("pencil", &salt, 4096),
        "SCRAM-SHA-256$4096:W22ZaJ0SNY7soEsUEjb6gQ==$\
         WG5d8oPm3OtcPnkdi4Uo7BkeZkBFzpcXkuLmtbsT4qY=:\
         wfPLwcE6nTWhTAmQ7tl2KeoiWGPlZqQxSrmfPwDl2dU="
    );
}

#[test]
fn the_role_and_databases_are_created_from_standard_input_only() {
    let (p, seen, root) = postgres("create", None);
    let dbs = ["bi_superset".to_string(), "bi_superset_cache".to_string()];
    p.create_module("bi_superset", &Secret::new(PASSWORD.into()), &dbs)
        .unwrap();
    let ran = seen.ran.borrow();
    assert_eq!(ran.len(), 1);
    let (cmd, sql) = &ran[0];
    let line = cmd.line();
    assert!(line.contains("exec -T postgres sh -c psql"), "{line}");
    assert!(
        !line.contains("SCRAM") && !line.contains(PASSWORD),
        "{line}"
    );
    assert!(
        !sql.contains(PASSWORD),
        "the clear password reached the server"
    );
    let verifier = sql
        .split("PASSWORD '")
        .nth(1)
        .and_then(|r| r.split('\'').next())
        .unwrap();
    let (head, keys) = verifier.split_once('$').unwrap().1.split_once('$').unwrap();
    assert!(
        head.starts_with("4096:") && keys.contains(':'),
        "{verifier}"
    );
    assert!(sql.contains(
        "NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT CONNECTION LIMIT 50"
    ));
    for db in &dbs {
        assert!(sql.contains(&format!("CREATE DATABASE \"{db}\" OWNER \"bi_superset\"")));
        assert!(sql.contains(&format!(
            "REVOKE CONNECT, TEMPORARY ON DATABASE \"{db}\" FROM PUBLIC"
        )));
        assert!(sql.contains(&format!(
            "GRANT CONNECT ON DATABASE \"{db}\" TO \"bi_superset\""
        )));
    }
    // Safe twice: created only when absent.
    assert_eq!(sql.matches("WHERE NOT EXISTS").count(), 3);
    // The script's file is gone.
    assert!(!root
        .join("var/lib/quality-data/updater/.module.sql")
        .exists());
    drop(ran);
    // Each run: a new salt, the same password.
    p.create_module("bi_superset", &Secret::new(PASSWORD.into()), &dbs)
        .unwrap();
    let ran = seen.ran.borrow();
    assert_ne!(ran[0].1, ran[1].1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn login_is_taken_away_with_its_sessions_and_given_back() {
    let (p, seen, root) = postgres("login", None);
    p.module_login("bi_superset", false).unwrap();
    p.module_login("bi_superset", true).unwrap();
    let ran = seen.ran.borrow();
    assert_eq!(
        ran[0].1,
        "ALTER ROLE \"bi_superset\" NOLOGIN;\nSELECT pg_terminate_backend(pid) \
         FROM pg_stat_activity WHERE usename = 'bi_superset';\n"
    );
    assert_eq!(ran[1].1, "ALTER ROLE \"bi_superset\" LOGIN;\n");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_purge_drops_the_databases_with_their_sessions_then_the_role() {
    let (p, seen, root) = postgres("drop", None);
    p.drop_module("bi_superset", &["bi_superset".into()])
        .unwrap();
    assert_eq!(
        seen.ran.borrow()[0].1,
        "DROP DATABASE IF EXISTS \"bi_superset\" WITH (FORCE);\nDROP ROLE IF EXISTS \"bi_superset\";\n"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_name_that_is_not_an_identifier_runs_nothing() {
    let (p, seen, root) = postgres("names", None);
    let pw = Secret::new(PASSWORD.into());
    for bad in ["", "Bi", "bi-superset", "bi;drop", "bi\"x", &"b".repeat(64)] {
        assert!(p.create_module(bad, &pw, &[]).is_err(), "{bad}");
        assert!(
            p.create_module("bi", &pw, &[bad.to_string()]).is_err(),
            "{bad}"
        );
        assert!(p.module_login(bad, true).is_err(), "{bad}");
        assert!(p.drop_module("bi", &[bad.to_string()]).is_err(), "{bad}");
    }
    assert!(seen.ran.borrow().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_failure_names_the_step_and_never_the_verifier() {
    let why = "psql:<stdin>:3: ERROR:  syntax error\nLINE 1: ALTER ROLE \"bi\" PASSWORD 'SCRAM-SHA-256$4096:x$y:z';";
    let (p, _, root) = postgres("fail", Some(why));
    let err = p
        .create_module("bi", &Secret::new(PASSWORD.into()), &[])
        .unwrap_err();
    assert!(
        err.starts_with("creating the role bi and its databases: psql:<stdin>:3: ERROR"),
        "{err}"
    );
    assert!(!err.contains("SCRAM") && !err.contains(PASSWORD), "{err}");
    assert!(!root
        .join("var/lib/quality-data/updater/.module.sql")
        .exists());
    let _ = std::fs::remove_dir_all(root);
}
