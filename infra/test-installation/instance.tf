# One EC2 instance standing in for an on-premises server: Ubuntu 24.04 LTS
# (Canonical's AMI), IMDSv2 required with hop limit 1, an encrypted gp3
# root volume. No instance profile: like a real installation, its only AWS
# credentials come from its Roles Anywhere enrolment certificate.
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

resource "aws_instance" "server" {
  ami                    = data.aws_ami.ubuntu.id
  instance_type          = var.instance_type
  subnet_id              = aws_subnet.public.id
  vpc_security_group_ids = [aws_security_group.test.id]
  key_name               = aws_key_pair.person.key_name

  metadata_options {
    http_endpoint               = "enabled"
    http_tokens                 = "required"
    http_put_response_hop_limit = 1
  }

  root_block_device {
    volume_type           = "gp3"
    volume_size           = var.volume_gb
    encrypted             = true
    delete_on_termination = true
  }

  # A newer AMI later does not replace a running test machine.
  lifecycle {
    ignore_changes = [ami]
  }

  tags = { Name = var.project }
}
