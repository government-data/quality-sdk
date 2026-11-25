# Update core in AWS

Decision 0001 (internal). Managed resources
only: S3 (index, packages, reports), ECR (images), KMS (index signature) and
IAM Roles Anywhere (who may pull), plus the Route 53 records of the public user
guide (decision 0010, `site.tf`; the hosted zone itself is only read). It lives beside the orchestrator's resources
in the same account with the `quality-data` prefix and reads, never creates, the
GitHub OIDC provider they share.

## Apply (a person, once)

```bash
cd infra/aws
cat > terraform.tfvars <<'EOF'
github_repo             = "owner/quality-sdk"
site_pages_verification = "..."   # see "The user guide's DNS" below
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
`government-data/core`, images `api` and `web`; `docs` → `government-data/docs`,
image `web`; `lake-rustfs` → `government-data/lake-rustfs`, image `admin`, not
the mirrored `rustfs` and `gate`) gets a role
`quality-data-publish-<module>` that GitHub Actions assumes on a `v*` tag of
that repository; it may push only to those ECR repositories. After applying, a
person sets two repository variables (Settings → Secrets and variables →
Actions → Variables; an ARN is not a secret) in each module repository:

```bash
gh variable set AWS_PUBLISH_ROLE --repo government-data/core \
  --body "$(terraform -chdir=infra/aws output -json module_publish_role_arns | jq -r '."quality-data"')"
gh variable set AWS_REGION --repo government-data/core --body us-east-1
```

Likewise for `government-data/docs` and `government-data/lake-rustfs` with the
output's `docs` and `lake-rustfs` keys.

## The user guide's DNS (`site.tf`)

The public user guide is served by GitHub Pages at `site_domain`
(`orquestador.app`). Terraform reads its Route 53 hosted zone (created by the
domain registration, managed by no Terraform) and manages four records:

- the apex: GitHub Pages' four `A` and four `AAAA` addresses;
- `www`: a `CNAME` to `<site_github_org>.github.io` (Pages redirects it to the
  apex);
- the `TXT` `_github-pages-challenge-<site_github_org>.<site_domain>` that
  verifies the domain for the organisation. It stays in DNS.

No wildcard (a takeover risk) and no `CAA` (one added later must allow
`letsencrypt.org`). `site_pages_verification` has no default: an organisation
owner adds the domain at
`https://github.com/organizations/<site_github_org>/settings/pages` and copies
the `TXT` value GitHub shows (there is no API for it) into `terraform.tfvars`.
After the apply, press "Verify" there.

### Taking the records over from the orchestrator's root (2026-11-25)

The records were created by `knime-orchestador/infra/aws`. The `import`
blocks in `site.tf` take them over without recreating them; that root forgets
them first with `removed { lifecycle { destroy = false } }`. In this order:

```bash
# 1. The orchestrator's root forgets the records (its plan: 4 "will no longer
#    be managed", "0 to add, 0 to change, 0 to destroy").
cd knime-orchestador/infra/aws
AWS_PROFILE=orquestador terraform plan -out forget.plan
AWS_PROFILE=orquestador terraform apply forget.plan

# 2. This root imports them; www moves to government-data.github.io and the
#    verification TXT of the old organisation name is replaced by the new one.
#    Expected: "Plan: 4 to import, 1 to add, 1 to change, 1 to destroy."
cd quality-sdk/infra/aws
AWS_PROFILE=orquestador terraform plan -out site.plan
AWS_PROFILE=orquestador terraform apply site.plan
```

Once applied, the `import` blocks are no-ops (the records are in this state).

## Renaming the GitHub organisation

A role with `ids` trusts GitHub's immutable subject under the name in its
`repo` and under every name in `github_owner_names` (by default
`government-data`), and the ids-only subject
`repository_owner_id:<owner id>:repository_id:<repo id>:ref:refs/tags/v*`
that a repository sends once its subject template is set to
`["repository_owner_id", "repository_id", "ref"]`. The ids say which
organisation and repository it is, so the trust works before and after a
rename. Apply this before renaming; drop the old name from the trust only
after a release has passed under the new one. The organisation was renamed
from `quality-data` to `government-data`, and the old name is no longer
trusted.

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
