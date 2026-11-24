//! A module's role and databases in the core's PostgreSQL (decision 0009,
//! point 5, as V2 verified), through the [`Postgres`] adapter: inside the
//! core's `postgres` container, as its superuser on the local socket.
//!
//! - The role, one per module: `LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE
//!   NOREPLICATION NOINHERIT CONNECTION LIMIT 50`, its password set from a
//!   **SCRAM-SHA-256 verifier computed here**, so the clear password never
//!   reaches the server, its log or a command line.
//! - Each database `CREATE DATABASE … OWNER <role>` when absent, then
//!   `REVOKE CONNECT, TEMPORARY … FROM PUBLIC` and `GRANT CONNECT` to the
//!   role.
//! - `remove`: `NOLOGIN` and its sessions ended; `add` again: `LOGIN`.
//! - `purge`: `DROP DATABASE … WITH (FORCE)` each, then `DROP ROLE`.
//!
//! Every script goes on psql's standard input (a `0600` file removed after
//! it ran), never in an argument; each statement is safe to run twice.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hmac::{Hmac, Mac};
use rand_core::RngCore;
use sha2::{Digest, Sha256};

use crate::module_secrets::Secret;
use crate::postgres::Postgres;

/// PostgreSQL's default SCRAM iteration count.
pub const SCRAM_ITERATIONS: u32 = 4096;
/// How many sessions a module's role may hold.
pub const CONNECTION_LIMIT: u32 = 50;

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[&[u8]]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
    for d in data {
        mac.update(d);
    }
    mac.finalize().into_bytes().into()
}

/// PBKDF2-HMAC-SHA-256, one block (RFC 8018).
fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut u = hmac(password, &[salt, &1u32.to_be_bytes()]);
    let mut out = u;
    for _ in 1..iterations {
        u = hmac(password, &[&u]);
        for (o, x) in out.iter_mut().zip(u) {
            *o ^= x;
        }
    }
    out
}

/// The verifier PostgreSQL keeps for `password` (RFC 5802 and 7677):
/// `SCRAM-SHA-256$<iterations>:<salt>$<StoredKey>:<ServerKey>`.
pub fn scram_verifier(password: &str, salt: &[u8], iterations: u32) -> String {
    let salted = pbkdf2(password.as_bytes(), salt, iterations);
    let client = hmac(&salted, &[b"Client Key"]);
    let stored = Sha256::digest(client);
    let server = hmac(&salted, &[b"Server Key"]);
    format!(
        "SCRAM-SHA-256${iterations}:{}${}:{}",
        STANDARD.encode(salt),
        STANDARD.encode(stored),
        STANDARD.encode(server)
    )
}

/// A role or database name the agent writes into SQL, always quoted:
/// `[a-z0-9_]`, at most 63 (the manifest's rule, checked again).
fn identifier(name: &str) -> Result<&str, String> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(name)
    } else {
        Err(format!("{name:?} cannot name a role or a database"))
    }
}

/// The statements of a module's role, its password and its databases.
fn create_sql(role: &str, verifier: &str, databases: &[&str]) -> String {
    let mut sql = format!(
        "SELECT 'CREATE ROLE \"{role}\"' WHERE NOT EXISTS \
         (SELECT FROM pg_roles WHERE rolname = '{role}')\\gexec\n\
         ALTER ROLE \"{role}\" LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION \
         NOINHERIT CONNECTION LIMIT {CONNECTION_LIMIT};\n\
         ALTER ROLE \"{role}\" PASSWORD '{verifier}';\n"
    );
    for db in databases {
        sql.push_str(&format!(
            "SELECT 'CREATE DATABASE \"{db}\" OWNER \"{role}\"' WHERE NOT EXISTS \
             (SELECT FROM pg_database WHERE datname = '{db}')\\gexec\n\
             REVOKE CONNECT, TEMPORARY ON DATABASE \"{db}\" FROM PUBLIC;\n\
             GRANT CONNECT ON DATABASE \"{db}\" TO \"{role}\";\n"
        ));
    }
    sql
}

impl Postgres {
    /// `sql` run by psql in the database's container, read from a `0600`
    /// file on its standard input, removed after.
    fn run_sql(&self, what: &str, sql: &str) -> Result<(), String> {
        let dir = self
            .backups
            .parent()
            .unwrap_or(&self.backups)
            .join("updater");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let file = dir.join(".module.sql");
        let _ = std::fs::remove_file(&file);
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file)
            .and_then(|mut f| f.write_all(sql.as_bytes()).and_then(|()| f.sync_all()));
        let ran = written
            .map_err(|e| format!("{}: {e}", file.display()))
            .and_then(|()| {
                let psql = r#"psql -U "$POSTGRES_USER" -d postgres -v ON_ERROR_STOP=1 -At"#;
                self.runner.run(&self.in_database(psql, &[])?.stdin(&file))
            });
        let _ = std::fs::remove_file(&file);
        // psql may quote the statement, which may hold the verifier: those
        // lines are left out.
        ran.map(|_| ()).map_err(|e| {
            let why: Vec<&str> = e
                .lines()
                .filter(|l| !l.contains("SCRAM-SHA-256$") && !l.contains("PASSWORD"))
                .collect();
            format!("{what}: {}", why.join(" "))
        })
    }

    /// The module's role with `password` (as a verifier) and `databases`
    /// owned by it, each closed to every other role.
    pub fn create_module(
        &self,
        role: &str,
        password: &Secret,
        databases: &[String],
    ) -> Result<(), String> {
        let role = identifier(role)?;
        let dbs = databases
            .iter()
            .map(|d| identifier(d))
            .collect::<Result<Vec<_>, _>>()?;
        let mut salt = [0u8; 16];
        rand_core::OsRng.fill_bytes(&mut salt);
        let verifier = scram_verifier(password.expose(), &salt, SCRAM_ITERATIONS);
        self.run_sql(
            &format!("creating the role {role} and its databases"),
            &create_sql(role, &verifier, &dbs),
        )
    }

    /// `LOGIN` again (`add` after `remove`), or `NOLOGIN` and every session
    /// of the role ended (`remove`).
    pub fn module_login(&self, role: &str, login: bool) -> Result<(), String> {
        let role = identifier(role)?;
        let sql = if login {
            format!("ALTER ROLE \"{role}\" LOGIN;\n")
        } else {
            format!(
                "ALTER ROLE \"{role}\" NOLOGIN;\n\
                 SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE usename = '{role}';\n"
            )
        };
        let what = if login { "LOGIN" } else { "NOLOGIN" };
        self.run_sql(&format!("{what} for the role {role}"), &sql)
    }

    /// `databases` dropped with their sessions, then the role (`purge`).
    pub fn drop_module(&self, role: &str, databases: &[String]) -> Result<(), String> {
        let role = identifier(role)?;
        let mut sql = String::new();
        for db in databases {
            let db = identifier(db)?;
            sql.push_str(&format!("DROP DATABASE IF EXISTS \"{db}\" WITH (FORCE);\n"));
        }
        sql.push_str(&format!("DROP ROLE IF EXISTS \"{role}\";\n"));
        self.run_sql(&format!("dropping the role {role} and its databases"), &sql)
    }
}

#[cfg(test)]
#[path = "module_database_tests.rs"]
mod tests;
