output "public_ip" {
  description = "The test machine's public address."
  value       = aws_instance.server.public_ip
}

output "ami" {
  description = "The Ubuntu 24.04 image it was started from."
  value       = aws_instance.server.ami
}

output "ssh" {
  description = "Log in, with the web of the installation tunnelled to https://localhost:8443."
  value       = "ssh -L 8443:localhost:443 ubuntu@${aws_instance.server.public_ip}"
}
