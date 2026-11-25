output "public_ips" {
  description = "Each machine's public address (SSH from ssh_cidr; the master's 443 too)."
  value       = { for name, m in aws_instance.machine : name => m.public_ip }
}

output "private_ips" {
  description = "Each machine's address inside the VPC; the clusters resolve app_domain and the internal name to the master's."
  value       = { for name, m in aws_instance.machine : name => m.private_ip }
}

output "ami" {
  description = "The Ubuntu 24.04 image they were started from."
  value       = aws_instance.machine["master"].ami
}

output "ssh" {
  description = "Log in to each machine as ubuntu."
  value       = { for name, m in aws_instance.machine : name => "ssh ubuntu@${m.public_ip}" }
}
