# Who may update (decision 0001): an installation with a valid certificate from
# the installations' CA. IAM Roles Anywhere trades the certificate for
# one-hour credentials of the installation role. Revoking is importing a CRL
# with the certificate's serial (`aws rolesanywhere import-crl`: the provider
# has no CRL resource), and the installation stops pulling within the hour.
#
# The certificate's subject names the installation: CN = its id, OU =
# "installation". Roles Anywhere passes them as principal tags.

resource "aws_rolesanywhere_trust_anchor" "installations" {
  name    = "${var.project}-installations"
  enabled = true
  source {
    source_type = "CERTIFICATE_BUNDLE"
    source_data {
      x509_certificate_data = var.installations_ca_pem
    }
  }
}

data "aws_iam_policy_document" "installation_trust" {
  statement {
    actions = ["sts:AssumeRole", "sts:TagSession", "sts:SetSourceIdentity"]
    principals {
      type        = "Service"
      identifiers = ["rolesanywhere.amazonaws.com"]
    }
    condition {
      test     = "ArnEquals"
      variable = "aws:SourceArn"
      values   = [aws_rolesanywhere_trust_anchor.installations.arn]
    }
    # A certificate from the same CA for another purpose does not serve.
    condition {
      test     = "StringEquals"
      variable = "aws:PrincipalTag/x509Subject/OU"
      values   = ["installation"]
    }
  }
}

resource "aws_iam_role" "installation" {
  name                 = "${var.project}-installation"
  assume_role_policy   = data.aws_iam_policy_document.installation_trust.json
  max_session_duration = var.installation_session_seconds
}

data "aws_iam_policy_document" "installation" {
  statement {
    sid       = "EcrLogin"
    actions   = ["ecr:GetAuthorizationToken"]
    resources = ["*"]
  }
  statement {
    sid = "PullImages"
    actions = [
      "ecr:BatchCheckLayerAvailability",
      "ecr:BatchGetImage",
      "ecr:GetDownloadUrlForLayer",
    ]
    resources = [for r in aws_ecr_repository.image : r.arn]
  }
  statement {
    sid       = "ReadIndexAndPackages"
    actions   = ["s3:GetObject"]
    resources = ["${aws_s3_bucket.releases.arn}/index/*", "${aws_s3_bucket.releases.arn}/packages/*"]
  }
  # Each installation writes only under its own id: one cannot report for
  # another, nor read anyone's reports.
  statement {
    sid       = "WriteOwnReports"
    actions   = ["s3:PutObject"]
    resources = ["${aws_s3_bucket.releases.arn}/reports/$${aws:PrincipalTag/x509Subject/CN}/*"]
  }
}

resource "aws_iam_role_policy" "installation" {
  role   = aws_iam_role.installation.id
  policy = data.aws_iam_policy_document.installation.json
}

resource "aws_rolesanywhere_profile" "installations" {
  name             = "${var.project}-installations"
  enabled          = true
  role_arns        = [aws_iam_role.installation.arn]
  duration_seconds = var.installation_session_seconds
}
