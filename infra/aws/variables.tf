variable "region" {
  description = "AWS region of the update core."
  type        = string
  default     = "us-east-1"
}

variable "project" {
  description = "Prefix of every resource. `quality-data` keeps it apart from the orchestrator's existing `orquestador-*` resources in the same account."
  type        = string
  default     = "quality-data"
}

variable "modules" {
  description = <<-EOT
    The images each module publishes: one ECR repository per pair, named
    <project>/<module>/<image>. Adding a module is one entry; removing one
    fails while its repositories still hold images (force_delete is off), so
    no published version disappears by mistake. Third-party images a module
    runs are mirrored into its own repositories by digest
    (tools/mirror-image.sh): quality-data's traefik and postgres.
  EOT
  type        = map(list(string))
  default = {
    "quality-data" = ["api", "web", "traefik", "postgres"]
    orchestrator   = ["api", "web"]
  }
}

variable "tool_images" {
  description = <<-EOT
    Third-party images the installation's agent itself runs, mirrored by
    digest (decision 0003): one ECR repository each, named
    <project>/tools/<image>. `busybox` empties a module's volume before its
    backup is restored into it, whatever the module's own image holds.
  EOT
  type        = list(string)
  default     = ["busybox"]
}

variable "versions_per_repository" {
  description = "Tagged images kept per ECR repository. Older versions can no longer be installed or rolled back to."
  type        = number
  default     = 30
}

variable "installations_ca_pem" {
  description = <<-EOT
    The PEM certificate of the CA that signs installation certificates
    (`quality-sdk enrol`). IAM Roles Anywhere trusts it: an installation with
    a valid certificate from it may pull updates. Its private key never
    reaches AWS or this state.
  EOT
  type        = string
  validation {
    condition     = startswith(trimspace(var.installations_ca_pem), "-----BEGIN CERTIFICATE-----")
    error_message = "installations_ca_pem must be a PEM certificate."
  }
}

variable "installation_session_seconds" {
  description = "How long the AWS credentials an installation gets last."
  type        = number
  default     = 3600
}

variable "github_repo" {
  description = "The GitHub repository that publishes versions, as `owner/repo`. Only its v* tags may publish."
  type        = string
}

variable "github_ids" {
  description = "Owner and repository ids, for GitHub's immutable OIDC subject. null: the subject by name only."
  type = object({
    owner = number
    repo  = number
  })
  default = null
}

variable "report_retention_days" {
  description = "Days the installations' update reports are kept."
  type        = number
  default     = 400
}

variable "module_publishers" {
  description = <<-EOT
    The GitHub repository whose v* tags push each module's images (decision
    0002 step 15): one role <project>-publish-<module> each, which may push
    only to the module's repositories named in `images` (the images it
    builds; mirrored third-party images are not the module role's) and has
    no S3 or KMS right. `ids` are the owner and repository ids for GitHub's
    immutable OIDC subject; null: the subject by name only.
  EOT
  type = map(object({
    repo   = string
    images = list(string)
    ids = optional(object({
      owner = number
      repo  = number
    }))
  }))
  default = {
    "quality-data" = { repo = "quality-data/core", images = ["api", "web"] }
  }
  validation {
    condition     = alltrue([for m, p in var.module_publishers : contains(keys(var.modules), m) && length(split("/", p.repo)) == 2 && length(setsubtract(p.images, lookup(var.modules, m, []))) == 0])
    error_message = "Each module_publishers key must be a module of var.modules, repo must be owner/name, and images must be that module's images."
  }
}
