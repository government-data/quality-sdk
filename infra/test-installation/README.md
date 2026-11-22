# Test installation in AWS

Decision 0004 (internal), point 5: one EC2 instance (Ubuntu 24.04,
`t3.large`, 40 GB encrypted `gp3`, IMDSv2 with hop limit 1) standing in for
an on-premises server, in its own VPC. SSH only, from one address; no
instance profile, so its only AWS credentials come from its Roles Anywhere
enrolment. Its state is local and apart from `infra/aws`.

Costs money while it runs (about 2.2 USD a day): destroy it after the test.

## Apply (a person)

```bash
cd infra/test-installation
cat > terraform.tfvars <<'EOF'
ssh_cidr       = "203.0.113.7/32"          # your address
ssh_public_key = "ssh-ed25519 AAAA... you" # your public key
EOF
AWS_PROFILE=orquestador terraform init
AWS_PROFILE=orquestador terraform plan -out test.plan
AWS_PROFILE=orquestador terraform apply test.plan
terraform output ssh
```

## Destroy (a person, after the test)

```bash
AWS_PROFILE=orquestador terraform destroy
```

Then revoke the installation's enrolment certificate.
