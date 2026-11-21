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

## Verify Roles Anywhere (a person, after applying)

```bash
QD_CA_CERT=~/qd-ca/ca.pem QD_CA_KEY=~/qd-ca/ca.key tools/verify/roles-anywhere.sh
```

Run from the repository root with the `orquestador` profile signed in. It
issues test certificates from the CA, checks what the installation role may
and may not do, revokes one with a CRL, prints PASS or FAIL per check and
removes everything it created.

## Revoke an installation

The provider has no CRL resource:

```bash
aws rolesanywhere import-crl --name quality-data-revoked --enabled \
  --trust-anchor-arn "$(terraform output -raw trust_anchor_arn)" \
  --crl-data fileb://revoked.crl
```

`quality-sdk revoke` will do this (decision 0001, step 6).
