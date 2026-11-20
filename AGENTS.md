# AGENTS.md

Instructions for coding agents working in this repository. Adapted from
the `quality-data` repository's `AGENTS.md` on 2026-11-20: the same way of
working, for this repo.

## What this is

quality-sdk: the core that installs, configures and updates Quality Data
(`quality-data`) and its modules (orchestrator, Grafana, RustFS, Trino) on local
installations, from versions published in AWS. One installation is one
organisation (no tenants) and there are no licenses: an installation may
update while its SDK enrolment is valid.

- `infra/aws/`: the AWS side, in Terraform (versions, images, who may pull).
- `crates/`: the SDK in Rust (manifest, update engine, agent, CLI), as the
  decisions add them.
- `docs/decisions/`: the design. Every change belongs to a decision.
  **Internal: ignored by git, never committed, pushed or published.**

## Before you change anything

1. **Find the decision.** Read its **Decision** and its **Implementation**
   section: it names the files, resources, crates and the check that says it
   is done.
2. **Follow the decision; do not redesign it in code.** If the code needs
   something the decision does not say, or contradicts it, stop and say so.
   An accepted decision changes only through a new record; a **proposed** one
   can be edited.
3. **Verify external behaviour first.** Before building on what AWS (IAM
   Roles Anywhere, KMS, ECR, S3), Docker Compose or PostgreSQL do, write or
   run a script in `tools/verify/` and record the result in the decision
   (date, version, what was found).

## Architecture patterns

- **Installations pull.** Nothing pushes to an installation: its agent reads
  the signed version index and pulls images and packages. No inbound port.
- **Identity by certificate, no long-lived secret.** An installation proves
  who it is with its enrolment certificate (IAM Roles Anywhere) and gets
  one-hour AWS credentials. Revoking the certificate stops its updates.
- **The index is signed.** Versions are trusted by the KMS signature on the
  index, never by where the file came from. The agent pins the public key.
- **Ports and adapters.** The update engine is a state machine behind a
  `Host` trait; AWS, Docker, PostgreSQL and files live in adapters. Every
  phase is tested without them.
- **Updates are steps that survive a crash.** The state is written before
  each step that changes something; every step can run twice.
- **Modules are removable.** A module is one folder with its manifest; the
  core never names a module. Removing a module keeps its data; only an
  explicit purge deletes it, after a backup.
- **Pure crates stay pure.** The manifest model does no I/O.

## Data rules

- **No data is lost by an update.** Back up and check the backup before
  applying; if a critical piece cannot be backed up, do not update.
- **Migrations only add.** Drops, renames, type changes and new required
  columns go in a later version, marked `-- contract: vX.Y.Z no longer uses
  it`; a version that cannot go back is marked `reversible: false`.
- **No personal data** in logs, reports to AWS, test fixtures, examples or
  documentation. A report carries versions, phases and errors, not data.
- **Secrets** never go in the repository, a log, a commit or Terraform
  variables files that are committed. `terraform.tfvars` and state are
  ignored.
- **Never real cloud accounts in development or tests.** Tests use fakes or
  an AWS emulator. `terraform apply` against the real account is run only by
  a person, never by an agent.

## Language

**English only. Nothing in Spanish**, with the one exception at the end:

- Code, identifiers, file and directory names, commands and their flags
  (`quality-data install`, not `quality-data instalar`), configuration keys
  and paths.
- Comments, documentation, decisions, READMEs and Terraform descriptions.
- Every text a person reads: CLI messages, errors, logs, notices and reports.
- Commit messages, branch names, pull requests and issues.

Product and module names are English too (`quality-data`, `orchestrator`).
The only Spanish allowed is an identifier that already exists outside this
repository and cannot be renamed from here, quoted literally: the AWS
profile `orquestador`, the legacy `orquestador-*` AWS resources, the
`knime-orchestador` repositories. Code brought from other repositories is
translated to English when it comes in.

**The one exception is documentation for people who use the product**: the
content of the `docs` module (user and administrator guide) and its
translations may be in Spanish. It covers that content only: the docs
module's code, configuration, build and comments, and the technical
documentation of every repository (decisions, READMEs, AGENTS.md), stay in
English.

## Size limits

Targets, then hard limits that fail CI:

- **Rust file** 400, hard 800 lines. **Terraform file** 250, hard 400.
- **Function** 60, hard 100 lines (Clippy `too_many_lines`).
- **Test files** 800, no hard limit: split by use case.
- **A change**: at most about 400 diff lines without generated files, one
  step of a decision's Implementation at a time.

Above the target, split when you touch it. Never add an exception: split.

## Checks before a commit

```sh
terraform -chdir=infra/aws fmt -check -recursive
terraform -chdir=infra/aws validate
# once there are crates, one crate at a time (see "Builds on this machine"):
cargo fmt --all -- --check
cargo clippy -p <crate> --all-targets --locked -- -D warnings
cargo test -p <crate> --locked
```

## Documentation with the code

- Update the decision's **Implementation** section in the same commit that
  changes it.
- A new decision follows the sections: Context, Decision, Implementation,
  Consequences, Open or Verification.

## Do not

- Do not add a service a decision does not name: the AWS side is S3, ECR,
  KMS and IAM Roles Anywhere until a decision says otherwise.
- Do not bring back licenses, plans or tenants.
- Do not commit, quote or link a decision in anything public: code
  comments may name it by number ("decision 0001"), never copy its content.
- Do not weaken a test, a lint or a check to make it pass.
- Do not touch the Terraform state of another project
  (`knime-orchestador*/infra/aws`): read it, never change it.
- Do not copy data from an organisation into the repository, a fixture or a
  prompt.

## Architecture
- all task always with subagents opus
- subagents always run with reasoning effort medium
- fable is orcherstator

## Builds on this machine

This machine is WSL with 16 GB and one disk. Builds saturate the disk's
reads and writes: on 2026-11-14 two sessions building at once, with a
four-node verification running, hung the disk and the machine restarted.
Every agent follows these rules, with no exception for "a quick check":

- **One build at a time, on the whole machine.** Before any `cargo` or
  `pnpm` command, check that no compiler is running (`pgrep -x rustc`,
  `pgrep -x cc1plus`) and wait if one is. Another session's build counts.
  Never use `pgrep -f`: it matches its own shell.
- **Every `cargo` and `pnpm` command goes through `~/.local/bin/capped`**,
  and `cargo` with `-j 2`.
- **Never `--workspace`.** Build, test and lint one crate at a time
  (`-p <crate>`), only the crates the change touches. Integration tests
  that start PostgreSQL run with `--test-threads=2`.
- **Do not run `cargo clean`, do not delete a `target` directory**, do not
  change features, profiles or `RUSTFLAGS`, and do not create a new
  `target` directory unless the user asked for it.
- **Verification scripts (`tools/verify/`) run alone**: never while a
  build runs, and one script at a time.
- **Clean up what you start.** Remove your `qs-verify-…` containers,
  networks and volumes when a script ends, also when it fails; remove them
  in a trap.
- **If the machine slows down** (load above 8 in `uptime`, or a command
  that does not answer), stop your own build by its PID and tell the user;
  do not start anything else.
