variable "region" {
  description = "Where the test machine runs: the release side's region, so pulls from ECR and S3 carry no transfer charge."
  type        = string
  default     = "us-east-1"
}

variable "project" {
  description = "Prefix of every name and the Project tag."
  type        = string
  default     = "quality-data-test-installation"
}

variable "ssh_cidr" {
  description = "The only address allowed in, on SSH (22): the person's, as a /32 (in an ignored terraform.tfvars)."
  type        = string
  validation {
    condition     = can(cidrhost(var.ssh_cidr, 0)) && !startswith(var.ssh_cidr, "0.0.0.0/")
    error_message = "ssh_cidr must be a CIDR block of the person's address, never 0.0.0.0/0."
  }
}

variable "ssh_public_key" {
  description = "The public key that may log in as ubuntu (an OpenSSH public key line; the private key stays with the person)."
  type        = string
}

variable "instance_type" {
  description = "The test machine's size: 2 vCPU and 8 GiB."
  type        = string
  default     = "t3.large"
}

variable "volume_gb" {
  description = "The root volume's size, gp3 and encrypted."
  type        = number
  default     = 40
}
