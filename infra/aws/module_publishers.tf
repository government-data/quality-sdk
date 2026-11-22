# Each module repository's CI pushes its own images (decision 0002 step 15):
# GitHub Actions on a v* tag of that repository assumes its module's role
# through OIDC. The role pushes only to the module's ECR repositories of the
# images it builds (not the mirrored third-party ones); it
# writes no package, index or signature, so a compromised module repository
# cannot reach an installation. The provider is the one publisher.tf reads.

locals {
  module_publisher_subjects = {
    for module, p in var.module_publishers : module => p.ids == null ? "repo:${p.repo}:ref:refs/tags/v*" : (
      "repo:${split("/", p.repo)[0]}@${p.ids.owner}/${split("/", p.repo)[1]}@${p.ids.repo}:ref:refs/tags/v*"
    )
  }
}

data "aws_iam_policy_document" "module_publish_trust" {
  for_each = var.module_publishers
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
      values   = [local.module_publisher_subjects[each.key]]
    }
  }
}

resource "aws_iam_role" "module_publish" {
  for_each             = var.module_publishers
  name                 = "${var.project}-publish-${each.key}"
  assume_role_policy   = data.aws_iam_policy_document.module_publish_trust[each.key].json
  max_session_duration = 3600
}

data "aws_iam_policy_document" "module_publish" {
  for_each = var.module_publishers
  statement {
    sid       = "EcrLogin"
    actions   = ["ecr:GetAuthorizationToken"]
    resources = ["*"]
  }
  # DescribeImages: the workflow skips a tag that already exists, since the
  # tags are immutable and builds are not reproducible.
  statement {
    sid = "PushOwnImages"
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
    resources = [for image in each.value.images : aws_ecr_repository.image["${each.key}/${image}"].arn]
  }
}

resource "aws_iam_role_policy" "module_publish" {
  for_each = var.module_publishers
  role     = aws_iam_role.module_publish[each.key].id
  policy   = data.aws_iam_policy_document.module_publish[each.key].json
}
