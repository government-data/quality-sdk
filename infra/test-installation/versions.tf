# The test installation (decision 0004 point 5): one machine in AWS standing
# in for an on-premises server. Its own root and its own local state, apart
# from infra/aws: destroying it never touches the release side. Applied and
# destroyed by a person, never by an agent.
terraform {
  required_version = ">= 1.6"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.70"
    }
  }
}

provider "aws" {
  region = var.region
  default_tags {
    tags = {
      Project   = var.project
      ManagedBy = "terraform"
      Purpose   = "test-installation"
    }
  }
}
