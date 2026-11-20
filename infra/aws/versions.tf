terraform {
  required_version = ">= 1.6"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.70"
    }
  }
  # Remote state, once its bucket and lock table exist (decision 0001, Open):
  # backend "s3" {
  #   bucket         = "quality-terraform-state"
  #   key            = "updates/terraform.tfstate"
  #   region         = "us-east-1"
  #   dynamodb_table = "quality-terraform-lock"
  #   encrypt        = true
  # }
}

provider "aws" {
  region = var.region
  default_tags {
    tags = {
      Project   = var.project
      ManagedBy = "terraform"
      Purpose   = "updates"
    }
  }
}
