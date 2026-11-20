# The release bucket (decision 0001), private and versioned:
#
#   index/<channel>.json       the versions a channel offers (signed)
#   index/<channel>.json.sig   its KMS signature
#   packages/<module>/<version>.tar.gz
#   reports/<installation>/... how each update ended, one object per outcome
#
# Versioning keeps every index ever published: going back to a previous index
# is restoring an object version, and the history says who published what.

data "aws_caller_identity" "current" {}

resource "aws_s3_bucket" "releases" {
  bucket = "${var.project}-releases-${data.aws_caller_identity.current.account_id}"
}

resource "aws_s3_bucket_public_access_block" "releases" {
  bucket                  = aws_s3_bucket.releases.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "releases" {
  bucket = aws_s3_bucket.releases.id
  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

resource "aws_s3_bucket_versioning" "releases" {
  bucket = aws_s3_bucket.releases.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "releases" {
  bucket = aws_s3_bucket.releases.id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

# HTTPS only.
resource "aws_s3_bucket_policy" "releases" {
  bucket = aws_s3_bucket.releases.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "HttpsOnly"
      Effect    = "Deny"
      Principal = "*"
      Action    = "s3:*"
      Resource  = [aws_s3_bucket.releases.arn, "${aws_s3_bucket.releases.arn}/*"]
      Condition = { Bool = { "aws:SecureTransport" = "false" } }
    }]
  })
}

resource "aws_s3_bucket_lifecycle_configuration" "releases" {
  bucket = aws_s3_bucket.releases.id
  rule {
    id     = "reports"
    status = "Enabled"
    filter {
      prefix = "reports/"
    }
    expiration {
      days = var.report_retention_days
    }
  }
  # Packages and indexes are never expired by age: a package is needed as long
  # as an installation may roll back to its version. Only overwritten copies go.
  rule {
    id     = "overwritten-copies"
    status = "Enabled"
    filter {
      prefix = "packages/"
    }
    noncurrent_version_expiration {
      noncurrent_days = 30
    }
  }
}
