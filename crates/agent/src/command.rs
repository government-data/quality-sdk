//! Running a program: the port the Docker Compose and PostgreSQL adapters
//! run `docker` through, so their tests use a fake ([`fake::Fake`]) and
//! only `tools/verify/` scripts run the real one.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A program to run, with its standard input and output from and to files
/// when they are given.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<PathBuf>,
    /// Written only by its owner (`0600`): a dump goes there.
    pub stdout: Option<PathBuf>,
}

impl Cmd {
    pub fn new(program: &str, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    pub fn stdin(mut self, file: &Path) -> Self {
        self.stdin = Some(file.into());
        self
    }

    pub fn stdout(mut self, file: &Path) -> Self {
        self.stdout = Some(file.into());
        self
    }

    /// The program and its arguments, as errors and fakes name it.
    pub fn line(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Runs a command: its standard output when it succeeds (empty when it
/// went to a file), or why it failed with the end of what it said.
pub trait Runner {
    fn run(&self, cmd: &Cmd) -> Result<String, String>;
}

/// The system's processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct System;

impl Runner for System {
    fn run(&self, cmd: &Cmd) -> Result<String, String> {
        use std::os::unix::fs::OpenOptionsExt;
        let what = || cmd.line().chars().take(120).collect::<String>();
        let mut c = Command::new(&cmd.program);
        c.args(&cmd.args).stderr(Stdio::piped());
        c.stdin(match &cmd.stdin {
            Some(f) => std::fs::File::open(f)
                .map_err(|e| format!("{}: {e}", f.display()))?
                .into(),
            None => Stdio::null(),
        });
        c.stdout(match &cmd.stdout {
            Some(f) => std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(f)
                .map_err(|e| format!("{}: {e}", f.display()))?
                .into(),
            None => Stdio::piped(),
        });
        let out = c
            .output()
            .map_err(|e| format!("could not run {}: {e}", what()))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        let said = String::from_utf8_lossy(&out.stderr);
        let said = said.trim();
        let tail: String = said
            .chars()
            .skip(said.chars().count().saturating_sub(600))
            .collect();
        Err(format!("{} failed ({}): {tail}", what(), out.status))
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::cell::RefCell;

    use super::{Cmd, Runner};

    /// Answers each command by the first rule whose text its line
    /// contains, `Ok("")` otherwise, and keeps every line it ran.
    #[derive(Debug, Default)]
    pub(crate) struct Fake {
        pub(crate) rules: Vec<(String, Result<String, String>)>,
        pub(crate) ran: RefCell<Vec<Cmd>>,
    }

    impl Fake {
        pub(crate) fn answer(mut self, contains: &str, answer: Result<&str, &str>) -> Self {
            let answer = answer.map(str::to_string).map_err(str::to_string);
            self.rules.push((contains.into(), answer));
            self
        }

        pub(crate) fn lines(&self) -> Vec<String> {
            self.ran.borrow().iter().map(Cmd::line).collect()
        }
    }

    impl Runner for Fake {
        fn run(&self, cmd: &Cmd) -> Result<String, String> {
            self.ran.borrow_mut().push(cmd.clone());
            let line = cmd.line();
            self.rules
                .iter()
                .find(|(contains, _)| line.contains(contains.as_str()))
                .map_or(Ok(String::new()), |(_, answer)| answer.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn a_command_reads_and_writes_files_and_a_failure_says_why() {
        let dir = std::env::temp_dir().join(format!("agent-command-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("in"), "dump bytes").unwrap();
        let _running = crate::fake_exec::running();
        let out = dir.join("out");
        let copied = System.run(&Cmd::new("cat", ["-"]).stdin(&dir.join("in")).stdout(&out));
        assert_eq!(copied.unwrap(), "");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "dump bytes");
        let mode = std::fs::metadata(&out).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(System.run(&Cmd::new("echo", ["hi"])).unwrap(), "hi\n");
        let err = System
            .run(&Cmd::new("sh", ["-c", "echo why >&2; exit 3"]))
            .unwrap_err();
        assert!(err.starts_with("sh -c") && err.ends_with(": why"), "{err}");
        assert!(System
            .run(&Cmd::new("/nonexistent/docker", ["ps"]))
            .unwrap_err()
            .contains("could not run"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_fake_answers_by_rule_and_keeps_what_it_ran() {
        let fake = fake::Fake::default()
            .answer(" ps", Ok("[]"))
            .answer(" up ", Err("no"));
        assert_eq!(
            fake.run(&Cmd::new("docker", ["compose", "ps"])).unwrap(),
            "[]"
        );
        assert_eq!(
            fake.run(&Cmd::new("docker", ["compose", "up", "-d"]))
                .unwrap_err(),
            "no"
        );
        assert_eq!(fake.run(&Cmd::new("docker", ["info"])).unwrap(), "");
        assert_eq!(
            fake.lines(),
            ["docker compose ps", "docker compose up -d", "docker info"]
        );
    }
}
