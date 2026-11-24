# Update core in AWS

Decision 0001 (internal). Managed resources
only: S3 (index, packages, reports), ECR (images), KMS (index signature) and
IAM Roles Anywhere (who may pull). It lives beside the orchestrator's resources
in the same account with the `quality-data` prefix and reads, never creates, the
GitHub OIDC provider they share.

## Apply (a person, once)

```bash
cd infra/aws
cat > terraform.tfvars <<'EOF'
github_repo          = "owner/quality-sdk"
installations_ca_pem = <<PEM
-----BEGIN CERTIFICATE-----
...the installations' CA certificate (its key stays offline)...
-----END CERTIFICATE-----
PEM
EOF
AWS_PROFILE=orquestador terraform init
AWS_PROFILE=orquestador terraform plan -out core.plan
AWS_PROFILE=orquestador terraform apply core.plan
```

The state is local until the remote one exists (decision 0001, Open); it holds
no secret, but keep a copy.

## A module repository's CI pushes its images

Each entry of `module_publishers` (by default `quality-data` →
`quality-data/core`, images `api` and `web`; `docs` → `quality-data/docs`,
image `web`; `lake-rustfs` → `quality-data/lake-rustfs`, image `admin`, not
the mirrored `rustfs` and `gate`) gets a role
`quality-data-publish-<module>` that GitHub Actions assumes on a `v*` tag of
that repository; it may push only to those ECR repositories. After applying, a
person sets two repository variables (Settings → Secrets and variables →
Actions → Variables; an ARN is not a secret) in each module repository:

```bash
gh variable set AWS_PUBLISH_ROLE --repo quality-data/core \
  --body "$(terraform -chdir=infra/aws output -json module_publish_role_arns | jq -r '."quality-data"')"
gh variable set AWS_REGION --repo quality-data/core --body us-east-1
```

Likewise for `quality-data/docs` and `quality-data/lake-rustfs` with the
output's `docs` and `lake-rustfs` keys.

## Renaming the GitHub organisation

A role with `ids` trusts GitHub's immutable subject under the name in its
`repo` and under every name in `github_owner_names` (by default
`quality-data` and `government-data`), and the ids-only subject
`repository_owner_id:<owner id>:repository_id:<repo id>:ref:refs/tags/v*`
that a repository sends once its subject template is set to
`["repository_owner_id", "repository_id", "ref"]`. The ids say which
organisation and repository it is, so the trust works before and after the
rename. Apply this before renaming; drop the old name from the trust only
after a release has passed under the new one.

## Verify Roles Anywhere (a person, after applying)

```bash
QD_CA_CERT=~/qd-ca/ca.pem QD_CA_KEY=~/qd-ca/ca.key tools/verify/roles-anywhere.sh
```

Run from the repository root with the `orquestador` profile signed in. It
issues test certificates from the CA, checks what the installation role may
and may not do, revokes one with a CRL, prints PASS or FAIL per check and
removes everything it created.

## Revoke an installation

```bash
qsdk revoke --ca-cert ~/qd-ca/ca.pem --ca-key ~/qd-ca/ca.key \
  --trust-anchor-arn "$(terraform -chdir=infra/aws output -raw trust_anchor_arn)" \
  --certificate installation.pem
```

The CRL file is the list of every revoked serial. It lives beside the CA key,
`~/qd-ca/installations.crl` by default (`--crl` to choose another), and is
backed up with the key: losing it means a new CRL without the earlier serials.
The person revoking needs the four CRL actions of the
`quality-data-enrolment-officers` group (`rolesanywhere:ListCrls`, `ImportCrl`,
`UpdateCrl`, `EnableCrl`): Terraform creates the group, not its members; a
person adds the people who revoke to it.
