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
    (tools/mirror-image.sh): quality-data's traefik, postgres and dolt (its
    catalog), lake-rustfs's rustfs and gate (Traefik in front of the lake;
    its own image is admin, the lake admin adapter), bi-superset's valkey
    (its cache and sessions; its own image is superset).
  EOT
  type        = map(list(string))
  default = {
    "quality-data" = ["api", "web", "traefik", "postgres", "dolt"]
    orchestrator   = ["api", "web"]
    docs           = ["web"]
    "lake-rustfs"  = ["rustfs", "gate", "admin"]
    "bi-superset"  = ["superset", "valkey"]
  }
  validation {
    condition     = !contains(keys(var.modules), "tools")
    error_message = "No module may be named tools: <project>/tools/* holds the agent's helper images (tool_images)."
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
    (`qsdk sign-enrolment`, for requests made with `qd enrol`). IAM Roles Anywhere trusts it: an installation with
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
  description = "Owner and repository ids, for GitHub's immutable OIDC subject (under every name in github_owner_names too) and the ids-only subject. null: the subject by name only."
  type = object({
    owner = number
    repo  = number
  })
  default = null
}

variable "github_owner_names" {
  description = <<-EOT
    The names the GitHub organisation is trusted under, besides the one in
    each `repo` (decision 0002): a role with ids trusts the immutable subject
    `repo:<name>@<owner id>/<repo>@<repo id>` for each name. The ids, not
    the names, say which organisation it is. The old name quality-data is
    no longer trusted since the rename to government-data.
  EOT
  type        = list(string)
  default     = ["government-data"]
  validation {
    condition     = alltrue([for n in var.github_owner_names : can(regex("^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?$", n))])
    error_message = "Each github_owner_names entry must be a GitHub organisation name: letters, digits and inner hyphens."
  }
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
    immutable OIDC subject (under every name in github_owner_names too)
    and the ids-only subject; null: the subject by name only.
  EOT
  type = map(object({
    repo   = string
    images = list(string)
    ids = optional(object({
      owner = number
      repo  = number
    }))
  }))
  # government-data/core, docs, lake-rustfs and bi-superset send GitHub's
  # immutable subject (owner 359174349; repositories 1465567877, 1467989556,
  # 1469349992 and 1471716471), so their roles must trust that form.
  default = {
    "quality-data" = {
      repo   = "government-data/core"
      images = ["api", "web"]
      ids    = { owner = 359174349, repo = 1465567877 }
    }
    docs = {
      repo   = "government-data/docs"
      images = ["web"]
      ids    = { owner = 359174349, repo = 1467989556 }
    }
    "lake-rustfs" = {
      repo   = "government-data/lake-rustfs"
      images = ["admin"]
      ids    = { owner = 359174349, repo = 1469349992 }
    }
    "bi-superset" = {
      repo   = "government-data/bi-superset"
      images = ["superset"]
      ids    = { owner = 359174349, repo = 1471716471 }
    }
  }
  validation {
    condition     = alltrue([for m, p in var.module_publishers : contains(keys(var.modules), m) && length(split("/", p.repo)) == 2 && length(setsubtract(p.images, lookup(var.modules, m, []))) == 0])
    error_message = "Each module_publishers key must be a module of var.modules, repo must be owner/name, and images must be that module's images."
  }
}

variable "site_domain" {
  description = "The domain of the public user guide on GitHub Pages (decision 0010). Its Route 53 hosted zone is in this account and is only read."
  type        = string
  default     = "orquestador.app"
}

variable "site_github_org" {
  description = "The GitHub organisation that publishes the user guide with Pages: www is a CNAME to <site_github_org>.github.io and the verification TXT is _github-pages-challenge-<site_github_org>.<site_domain>."
  type        = string
  default     = "government-data"
  validation {
    condition     = can(regex("^[A-Za-z0-9](-?[A-Za-z0-9])*$", var.site_github_org))
    error_message = "site_github_org must be a GitHub organisation name (letters, digits and hyphens), without .github.io."
  }
}

variable "site_pages_verification" {
  description = <<-EOT
    The value of the TXT record _github-pages-challenge-<site_github_org>.<site_domain>
    that verifies the domain for the organisation. GitHub shows it to an
    organisation owner under the organisation's Settings → Pages → Add a
    domain (https://github.com/organizations/<site_github_org>/settings/pages);
    there is no API for it. Not a secret, but set it in terraform.tfvars
    (ignored by git), not in a committed file.
  EOT
  type        = string
  nullable    = false
  validation {
    condition     = length(trimspace(var.site_pages_verification)) > 0
    error_message = "site_pages_verification must be the TXT value GitHub shows in the organisation's Pages settings."
  }
}
