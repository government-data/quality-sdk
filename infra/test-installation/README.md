# Test installation in AWS

Decision 0004 (internal), point 5, and decision 0012 step 11: three EC2
instances in one VPC standing in for an installation, all Ubuntu 24.04,
IMDSv2 with hop limit 1, encrypted `gp3` volumes:

| Machine | Default size | Disks |
| --- | --- | --- |
| `master`: the core, the Trino coordinator, two job runners, the agent | `m6i.2xlarge` | 100 GB root |
| `lake`: one RustFS node | `m6i.large` | 40 GB root, four 50 GB data disks (attached as `/dev/sdf` to `/dev/sdi`, seen as NVMe disks on these types), one drive each |
| `trino-worker`: one Trino worker | `r6i.xlarge` | 40 GB root |

Sizes, disks and ports are variables (`machines`, `ports`). No instance
profile: the master's only AWS credentials come from its Roles Anywhere
enrolment; the clusters do not enrol. Its state is local and apart from
`infra/aws`.

One security group per machine, rules by source group (`rules.tf`): SSH
from `ssh_cidr` to each; the master's 443 from `ssh_cidr` and from both
clusters (its internal routes and the lake gate's check are on 443 by host
name); the Trino coordinator's port (8490, engine-trino's `TRINO_PORT`)
from the worker; the worker's one port, 443 (Trino with TLS), from the
master; the lake's one port, 443 (its gate and its admin adapter behind
it), from the master and the worker; no PostgreSQL port. Out: 443 and 80
to the internet, and the ports above to their groups. On the clusters cloud-init adds `app_domain`
and the internal name (`internal.<app_domain>` unless set) to `/etc/hosts`
as the master's private address; on the master and the worker it adds the
lake's name (`lake_name`, `lake-1` unless set: the name the lake joins
with) and `admin.<lake_name>` (its admin adapter, behind the same 443) as
the lake's private address. The public record for the browser is the
person's.

Costs money while it runs: destroy it after the test.

## Apply (a person)

```bash
cd infra/test-installation
cat > terraform.tfvars <<'EOT'
ssh_cidr       = "203.0.113.7/32"          # your address
ssh_public_key = "ssh-ed25519 AAAA... you" # your public key
app_domain     = "quality.example.org"     # the master's APP_DOMAIN
EOT
AWS_PROFILE=orquestador terraform init
AWS_PROFILE=orquestador terraform plan -out test.plan
AWS_PROFILE=orquestador terraform apply test.plan
terraform output ssh
```

Check: the plan adds only the three instances, their disks, groups and
rules, and destroys nothing it did not make. After the apply, from each
cluster `curl` reaches the master's 443 by `app_domain`, the worker also
the coordinator's 8490, the master the worker's 443 and the lake's 443,
and a port not listed is refused.

## Destroy (a person, after the test)

```bash
AWS_PROFILE=orquestador terraform destroy
```

Then revoke the master's enrolment certificate.
