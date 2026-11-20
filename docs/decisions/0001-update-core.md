---
title: "0001 — The update core: signed versions in AWS, installations that pull"
status: proposed
date: 2026-11-20
---

Status: proposed

## Context

Quality Data (`quality-data`) runs on local installations, together with its
modules (orchestrator, Grafana, RustFS, Trino) in one Docker Compose. People
reach every module from quality-data, signing in with OIDC. Updates bring new
container versions, database migrations and security patches; none may lose
data, and a failed update must recover by itself.

Until now this lived in the orchestrator's licensing cloud: a service with
keys, plans, permits and payments decided which version each installation
got. Licenses and tenants are dropped. What remains is the question licensing
answered on the side: **who may update**. The answer is now: an installation
that has the SDK installed and enrolled, until its enrolment is revoked.

The update daemon (`crates/updater` in the `quality-data` repository) already
does the local part well
(prepare, drain, back up, apply, verify, roll back, restore, critical) and is
kept; its source of versions changes.

## Decision

### No service of our own in AWS

The AWS side is managed resources only, in Terraform (`infra/aws/`):

| Resource | For |
| --- | --- |
| S3 `<project>-releases-<account>`, versioned | `index/`, `packages/`, `reports/` |
| ECR `<project>/<module>/<image>`, immutable tags | the images, pulled by digest |
| KMS key, ECC P-256, sign only | signs the index |
| IAM Roles Anywhere trust anchor + profile | an installation's certificate → 1 h credentials |
| IAM role `installation` | pull images, read index and packages, write its own reports |
| IAM role `publish`, GitHub OIDC on `v*` tags | push images and packages, sign and write the index |

### Who may update: the enrolment certificate

`quality-sdk enrol` creates a key on the installation's server and a
certificate request; we sign it with the installations' CA (subject CN = the
installation's id, OU = `installation`). The CA's certificate is the trust
anchor; its private key stays offline, out of AWS and of the Terraform state.
The agent trades the certificate for one-hour credentials of the
`installation` role. **Revoking** is importing a CRL with the certificate's
serial; the installation stops pulling within the hour. An installation can
write reports only under `reports/<its CN>/`.

Inside an installation, who may approve or start an update is quality-data's
permission (administrators by default) and `sudo` on the server, with the
update policy that exists today (manual, approval, automatic in a window).

### What a version is: the signed index

`index/<channel>.json` lists **platform versions**: quality-data plus the
version of each module, tested together, every image by digest, every
package with its SHA-256. Each carries `upgrade_from`, `reversible`, its
rollout (`pilot`, a percentage, or `all`) and `paused`.
`index/<channel>.json.sig` is its KMS
signature; the agent verifies it with the public key pinned at enrolment and
**never trusts an index that does not verify**, nor one older than the one it
last saw (no downgrade by replaying an old index).

The agent decides its own stage: it hashes its installation id into 0–99 and
takes a version when its stage reaches it. Pausing a version is publishing an
index with `paused: true`.

### Channels

`stable` for platform versions; `patch` for rebuilt images of the same
versions (security fixes, no migrations), which may carry `urgent` to skip
the maintenance window.

## Implementation

1. **Terraform of the core** (`infra/aws/`): bucket, repositories, signing
   key, trust anchor, profile, roles. Done when `terraform validate` passes
   and a person applies it. *(this step)*
2. **Verify Roles Anywhere** (`tools/verify/roles-anywhere.sh`): a test CA, a
   certificate with OU `installation`, `aws_signing_helper`, read `index/`,
   write own `reports/`, refused on another's; revoke with a CRL and see the
   refusal. Recorded below.
3. **`index-model` crate**: the index and the platform version, pure, with
   signature verification and the rollout stage; no I/O.
4. **Extract the update engine** from `crates/updater` of the `quality-data`
   repository into this
   repo, unchanged, behind its `Host` trait.
5. **AWS adapter of the agent**: credentials through Roles Anywhere, read and
   verify the index, pull by digest, write reports.
6. **`quality-sdk` CLI**: `enrol`, `sign-enrolment`, `revoke`, `publish`.
7. **The brake**: count failed reports per version and pause it (a new
   decision chooses where it runs).

## Consequences

- No server to keep up, patch or back up in AWS; the cost is S3, ECR storage
  and one KMS key.
- Rollout and brake logic move to the agent and the CLI; they must be tested
  there, since no service can correct them afterwards.
- Losing the installations' CA key means re-enrolling everyone; it is kept
  offline with a backup.
- Removing a module from `var.modules` fails while its repositories hold
  images: deleting published versions is a deliberate step.

## Open

- Remote Terraform state (S3 + lock) before a second person applies.
- Third-party images (Grafana, RustFS, Trino): mirror them into ECR by digest
  or pull from their registries.
- Where the brake runs (step 7).

## Verification

- 2026-11-20, `hashicorp/aws` ~> 5.70: the provider has
  `aws_rolesanywhere_trust_anchor` and `aws_rolesanywhere_profile` but no CRL
  resource; revocation goes through `aws rolesanywhere import-crl`.
- Pending (step 2): the principal tags `x509Subject/CN` and `x509Subject/OU`
  in the trust and resource policies, against the real service.
