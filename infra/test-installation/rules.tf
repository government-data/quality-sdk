# One security group per machine (decision 0012 step 11), rules by source
# group and nothing wider:
#
# - SSH (22) to each machine, from ssh_cidr only; the master's web entry
#   point (443) from ssh_cidr too, for the person's browser;
# - the master's one port (443) from both clusters: the web, the join, and
#   by host name the internal routes (renewal, the engine port) and the
#   core's lake check, which the lake's gate asks before each request;
# - the Trino coordinator's HTTP port (discovery) on the master from the
#   worker; the worker's HTTP port from the master;
# - the lake's one port (443: its gate, S3 and the Iceberg REST catalog,
#   and its admin adapter, routed inside by host name or path) from the
#   master and the worker;
# - no PostgreSQL port: no cluster claims from the queue (the job runners
#   are on the master);
# - out: HTTPS and HTTP to the internet (ECR, S3, packages), and each port
#   above to the group it goes to.
resource "aws_security_group" "machine" {
  for_each    = var.machines
  name        = "${var.project}-${each.key}"
  description = "Test installation ${each.key}: SSH from one address; the listed ports from the other machines' groups"
  vpc_id      = aws_vpc.test.id
  tags        = { Name = "${var.project}-${each.key}" }
}

locals {
  # Between the machines: from the group of `from` to `port` on `to`.
  flows = {
    "lake-web"           = { from = "lake", to = "master", port = var.ports.web }
    "trino-worker-web"   = { from = "trino-worker", to = "master", port = var.ports.web }
    "worker-coordinator" = { from = "trino-worker", to = "master", port = var.ports.coordinator }
    "master-worker"      = { from = "master", to = "trino-worker", port = var.ports.worker }
    "master-lake"        = { from = "master", to = "lake", port = var.ports.lake }
    "worker-lake"        = { from = "trino-worker", to = "lake", port = var.ports.lake }
  }
  # From the person's address.
  person = merge(
    { for name in keys(var.machines) : "${name}-ssh" => { to = name, port = 22 } },
    { "master-web" = { to = "master", port = var.ports.web } },
  )
  # Out to the internet, from each machine.
  internet = merge([
    for name in keys(var.machines) : {
      "${name}-https" = { from = name, port = 443 }
      "${name}-http"  = { from = name, port = 80 }
    }
  ]...)
}

resource "aws_vpc_security_group_ingress_rule" "person" {
  for_each          = local.person
  security_group_id = aws_security_group.machine[each.value.to].id
  description       = "${each.value.port} from the one allowed address"
  cidr_ipv4         = var.ssh_cidr
  ip_protocol       = "tcp"
  from_port         = each.value.port
  to_port           = each.value.port
}

resource "aws_vpc_security_group_ingress_rule" "flow" {
  for_each                     = local.flows
  security_group_id            = aws_security_group.machine[each.value.to].id
  description                  = "${each.value.port} from ${each.value.from}"
  referenced_security_group_id = aws_security_group.machine[each.value.from].id
  ip_protocol                  = "tcp"
  from_port                    = each.value.port
  to_port                      = each.value.port
}

resource "aws_vpc_security_group_egress_rule" "flow" {
  for_each                     = local.flows
  security_group_id            = aws_security_group.machine[each.value.from].id
  description                  = "${each.value.port} to ${each.value.to}"
  referenced_security_group_id = aws_security_group.machine[each.value.to].id
  ip_protocol                  = "tcp"
  from_port                    = each.value.port
  to_port                      = each.value.port
}

resource "aws_vpc_security_group_egress_rule" "internet" {
  for_each          = local.internet
  security_group_id = aws_security_group.machine[each.value.from].id
  description       = "${each.value.port} out to the internet"
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "tcp"
  from_port         = each.value.port
  to_port           = each.value.port
}
