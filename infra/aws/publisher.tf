# Publishing a version (decision 0001): GitHub Actions on a v* tag assumes this
# role through OIDC, with no AWS keys stored in GitHub. It pushes images,
# uploads packages, signs the index with KMS and writes it.
#
# GitHub's OIDC provider already exists in this account (one per URL), created
# by the orchestrator's Terraform: it is read, not created.

data "aws_iam_openid_connect_provider" "github" {
  url = "https://token.actions.githubusercontent.com"
}

locals {
  github_owner = split("/", var.github_repo)[0]
  github_name  = split("/", var.github_repo)[1]
  # As module_publishers.tf: with ids, every organisation name and ids only.
  github_subjects = var.github_ids == null ? ["repo:${var.github_repo}:ref:refs/tags/v*"] : concat(
    [
      for owner in distinct(concat([local.github_owner], var.github_owner_names)) :
      "repo:${owner}@${var.github_ids.owner}/${local.github_name}@${var.github_ids.repo}:ref:refs/tags/v*"
    ],
    ["repository_owner_id:${var.github_ids.owner}:repository_id:${var.github_ids.repo}:ref:refs/tags/v*"],
  )
}

data "aws_iam_policy_document" "publish_trust" {
  statement {
    actions = ["sts:AssumeRoleWithWebIdentity"]
    principals {
      type        = "Federated"
      identifiers = [data.aws_iam_openid_connect_provider.github.arn]
    }
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }
    condition {
      test     = "StringLike"
      variable = "token.actions.githubusercontent.com:sub"
      values   = local.github_subjects
    }
  }
}

resource "aws_iam_role" "publish" {
  name                 = "${var.project}-publish"
  assume_role_policy   = data.aws_iam_policy_document.publish_trust.json
  max_session_duration = 3600
}

data "aws_iam_policy_document" "publish" {
  statement {
    sid       = "EcrLogin"
    actions   = ["ecr:GetAuthorizationToken"]
    resources = ["*"]
  }
  statement {
    sid = "PushImages"
    actions = [
      "ecr:BatchCheckLayerAvailability",
      "ecr:BatchGetImage",
      "ecr:CompleteLayerUpload",
      "ecr:DescribeImages",
      "ecr:GetDownloadUrlForLayer",
      "ecr:InitiateLayerUpload",
      "ecr:PutImage",
      "ecr:UploadLayerPart",
    ]
    resources = [for r in aws_ecr_repository.image : r.arn]
  }
  statement {
    sid       = "WritePackagesAndIndex"
    actions   = ["s3:PutObject", "s3:GetObject"]
    resources = ["${aws_s3_bucket.releases.arn}/packages/*", "${aws_s3_bucket.releases.arn}/index/*"]
  }
  # To see how a version is going before widening its rollout.
  statement {
    sid       = "ReadReports"
    actions   = ["s3:GetObject"]
    resources = ["${aws_s3_bucket.releases.arn}/reports/*"]
  }
  statement {
    sid       = "ListBucket"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.releases.arn]
  }
  statement {
    sid       = "SignIndex"
    actions   = ["kms:Sign", "kms:GetPublicKey"]
    resources = [aws_kms_key.index_signing.arn]
  }
}

resource "aws_iam_role_policy" "publish" {
  role   = aws_iam_role.publish.id
  policy = data.aws_iam_policy_document.publish.json
}
