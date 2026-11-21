# Revoking an installation (decision 0001): `quality-sdk revoke` signs the
# installations' CRL offline and puts it into Roles Anywhere as the trust
# anchor's CRL. The people who revoke belong to this group; Terraform creates
# the group and its policy, never its members.
#
# ListCrls and ImportCrl take no resource (a CRL does not exist before it is
# imported); UpdateCrl and EnableCrl are scoped to this account's and region's
# CRLs, the tightest Roles Anywhere allows (its service authorization
# reference, read 2026-11-21).

data "aws_region" "current" {}

resource "aws_iam_group" "enrolment_officers" {
  name = "${var.project}-enrolment-officers"
}

data "aws_iam_policy_document" "enrolment_officers" {
  statement {
    sid       = "FindAndImportCrl"
    actions   = ["rolesanywhere:ListCrls", "rolesanywhere:ImportCrl"]
    resources = ["*"]
  }
  statement {
    sid     = "UpdateCrl"
    actions = ["rolesanywhere:UpdateCrl", "rolesanywhere:EnableCrl"]
    resources = [
      "arn:aws:rolesanywhere:${data.aws_region.current.name}:${data.aws_caller_identity.current.account_id}:crl/*",
    ]
  }
}

resource "aws_iam_group_policy" "enrolment_officers" {
  name   = "${var.project}-revoke-installations"
  group  = aws_iam_group.enrolment_officers.name
  policy = data.aws_iam_policy_document.enrolment_officers.json
}
