# Three EC2 instances in one VPC standing in for an installation's master
# server and two clusters (decision 0012): Ubuntu 24.04 LTS (Canonical's
# AMI), IMDSv2 required with hop limit 1, encrypted gp3 volumes. No
# instance profile: the master's only AWS credentials come from its Roles
# Anywhere enrolment; the clusters do not enrol and pull with credentials
# the core hands out over mTLS.
data "aws_ami" "ubuntu" {
  most_recent = true
  owners      = ["099720109477"] # Canonical
  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*"]
  }
  filter {
    name   = "architecture"
    values = ["x86_64"]
  }
}

resource "aws_key_pair" "person" {
  key_name   = var.project
  public_key = var.ssh_public_key
}

locals {
  internal_domain = var.internal_domain != "" ? var.internal_domain : "internal.${var.app_domain}"
  # Fixed private addresses, so each machine's /etc/hosts names the others
  # before they exist.
  master_ip = cidrhost(aws_subnet.public.cidr_block, 10)
  lake_ip   = cidrhost(aws_subnet.public.cidr_block, 11)
  # APP_DOMAIN and the internal name are the master's private address, so
  # the join and renewal stay inside the VPC; the lake's name (its S3 and
  # catalog) and admin.<its name> (its admin adapter, decision 0091 ruling
  # 72) are the lake's, on its one port.
  hosts = {
    master       = "${local.lake_ip} ${var.lake_name} admin.${var.lake_name}"
    lake         = "${local.master_ip} ${var.app_domain} ${local.internal_domain}"
    trino-worker = <<-EOT
      ${local.master_ip} ${var.app_domain} ${local.internal_domain}
      ${local.lake_ip} ${var.lake_name} admin.${var.lake_name}
    EOT
  }
  private_ip = { master = local.master_ip, lake = local.lake_ip }
}

resource "aws_instance" "machine" {
  for_each               = var.machines
  ami                    = data.aws_ami.ubuntu.id
  instance_type          = each.value.instance_type
  subnet_id              = aws_subnet.public.id
  private_ip             = lookup(local.private_ip, each.key, null)
  vpc_security_group_ids = [aws_security_group.machine[each.key].id]
  key_name               = aws_key_pair.person.key_name
  user_data = join("\n", [
    "#cloud-config",
    "write_files:",
    "  - path: /etc/hosts",
    "    append: true",
    "    content: |",
    "      ${indent(6, trimspace(local.hosts[each.key]))}",
    "",
  ])

  metadata_options {
    http_endpoint               = "enabled"
    http_tokens                 = "required"
    http_put_response_hop_limit = 1
  }

  root_block_device {
    volume_type           = "gp3"
    volume_size           = each.value.root_gb
    encrypted             = true
    delete_on_termination = true
  }

  # A newer AMI later does not replace a running test machine.
  lifecycle {
    ignore_changes = [ami]
  }

  tags = { Name = "${var.project}-${each.key}" }
}

locals {
  # One entry per data disk: "lake-1" to "lake-4", attached as /dev/sdf on.
  data_disks = merge([
    for name, m in var.machines : {
      for i in range(m.data_disks) : "${name}-${i + 1}" => {
        machine = name
        gb      = m.data_disk_gb
        device  = "/dev/sd${substr("fghijklm", i, 1)}"
      }
    }
  ]...)
}

resource "aws_ebs_volume" "data" {
  for_each          = local.data_disks
  availability_zone = aws_subnet.public.availability_zone
  type              = "gp3"
  size              = each.value.gb
  encrypted         = true
  tags              = { Name = "${var.project}-${each.key}" }
}

resource "aws_volume_attachment" "data" {
  for_each    = local.data_disks
  device_name = each.value.device
  volume_id   = aws_ebs_volume.data[each.key].id
  instance_id = aws_instance.machine[each.value.machine].id
}
