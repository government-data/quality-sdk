# Its own VPC with one public subnet. Inbound: SSH only, from ssh_cidr; 443
# and 9443 are not opened (the web is reached through an SSH tunnel).
# Outbound: HTTPS (the AWS endpoints) and HTTP (Ubuntu's and Docker's
# packages). The VPC's DNS resolver and time service are not filtered by
# security groups.
data "aws_availability_zones" "available" {
  state = "available"
}

resource "aws_vpc" "test" {
  cidr_block           = "10.80.0.0/24"
  enable_dns_support   = true
  enable_dns_hostnames = true
  tags                 = { Name = var.project }
}

resource "aws_internet_gateway" "test" {
  vpc_id = aws_vpc.test.id
  tags   = { Name = var.project }
}

resource "aws_subnet" "public" {
  vpc_id                  = aws_vpc.test.id
  cidr_block              = "10.80.0.0/25"
  availability_zone       = data.aws_availability_zones.available.names[0]
  map_public_ip_on_launch = true
  tags                    = { Name = "${var.project}-public" }
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.test.id
  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.test.id
  }
  tags = { Name = "${var.project}-public" }
}

resource "aws_route_table_association" "public" {
  subnet_id      = aws_subnet.public.id
  route_table_id = aws_route_table.public.id
}

resource "aws_security_group" "test" {
  name        = var.project
  description = "Test installation: SSH in from one address; HTTPS and HTTP out"
  vpc_id      = aws_vpc.test.id
  tags        = { Name = var.project }
}

resource "aws_vpc_security_group_ingress_rule" "ssh" {
  security_group_id = aws_security_group.test.id
  description       = "SSH from the one allowed address"
  cidr_ipv4         = var.ssh_cidr
  ip_protocol       = "tcp"
  from_port         = 22
  to_port           = 22
}

resource "aws_vpc_security_group_egress_rule" "out" {
  for_each          = { https = 443, http = 80 }
  security_group_id = aws_security_group.test.id
  description       = "${upper(each.key)} out"
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "tcp"
  from_port         = each.value
  to_port           = each.value
}
