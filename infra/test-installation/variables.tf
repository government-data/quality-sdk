variable "region" {
  description = "Where the test machines run: the release side's region, so pulls from ECR and S3 carry no transfer charge."
  type        = string
  default     = "us-east-1"
}

variable "project" {
  description = "Prefix of every name and the Project tag."
  type        = string
  default     = "quality-data-test-installation"
}

variable "ssh_cidr" {
  description = "The only address allowed in, on SSH (22) to each machine and HTTPS (443) to the master: the person's, as a /32 (in an ignored terraform.tfvars)."
  type        = string
  validation {
    condition     = can(cidrhost(var.ssh_cidr, 0)) && !startswith(var.ssh_cidr, "0.0.0.0/")
    error_message = "ssh_cidr must be a CIDR block of the person's address, never 0.0.0.0/0."
  }
}

variable "ssh_public_key" {
  description = "The public key that may log in as ubuntu on each machine (an OpenSSH public key line; the private key stays with the person)."
  type        = string
}

variable "machines" {
  description = "The three machines (decision 0012): each one's size, root volume and data disks (gp3, encrypted, one size each). The master runs the core with the Trino coordinator and two job runners; the lake has four data disks, one RustFS drive each; the Trino worker is sized for memory."
  type = map(object({
    instance_type = string
    root_gb       = number
    data_disks    = number
    data_disk_gb  = number
  }))
  default = {
    master       = { instance_type = "m6i.2xlarge", root_gb = 100, data_disks = 0, data_disk_gb = 0 }
    lake         = { instance_type = "m6i.large", root_gb = 40, data_disks = 4, data_disk_gb = 50 }
    trino-worker = { instance_type = "r6i.xlarge", root_gb = 40, data_disks = 0, data_disk_gb = 0 }
  }
  validation {
    condition     = toset(keys(var.machines)) == toset(["master", "lake", "trino-worker"])
    error_message = "machines names exactly master, lake and trino-worker."
  }
  validation {
    condition     = alltrue([for m in values(var.machines) : m.data_disks >= 0 && m.data_disks <= 8 && (m.data_disks == 0 || m.data_disk_gb >= 1)])
    error_message = "Each machine has 0 to 8 data disks, each of at least 1 GB."
  }
}

variable "app_domain" {
  description = "The master's APP_DOMAIN (the web entry point and the join). On the clusters it resolves to the master's private address; the public record for the person's browser is the person's."
  type        = string
}

variable "internal_domain" {
  description = "The master's internal name (INTERNAL_DOMAIN); empty: internal.<app_domain>, the core's default. Resolves on the clusters as app_domain does."
  type        = string
  default     = ""
}

variable "lake_name" {
  description = "The lake machine's name, the one it joins with and its certificate carries; the master and the worker resolve it and admin.<it> to the lake's private address."
  type        = string
  default     = "lake-1"
  validation {
    condition     = can(regex("^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$", var.lake_name))
    error_message = "lake_name is a machine name: lowercase letters, digits and hyphens, 1 to 63."
  }
}

variable "ports" {
  description = "The ports between the machines, as decisions 0011 and 0012 steps 9 and 10 publish them: the master's one port (443: the web, the join, and by host name its internal routes and the lake gate's check), the Trino coordinator's and the worker's HTTP ports, and the lake's one port (443: its gate, S3 and the Iceberg REST catalog, and its admin adapter, routed inside by host name or path)."
  type = object({
    web         = number
    coordinator = number
    worker      = number
    lake        = number
  })
  default = {
    web         = 443
    coordinator = 8080
    worker      = 8080
    lake        = 443
  }
}
