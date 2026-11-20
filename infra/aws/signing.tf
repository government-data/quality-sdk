# The key that signs the version index (decision 0001). It never leaves KMS:
# publishing signs through the publish role, and installations verify with the
# public key pinned in the agent, never fetched from the same channel as the
# index. ECDSA P-256 with SHA-256.

resource "aws_kms_key" "index_signing" {
  description              = "${var.project}: signs the update index"
  key_usage                = "SIGN_VERIFY"
  customer_master_key_spec = "ECC_NIST_P256"
  deletion_window_in_days  = 30
}

resource "aws_kms_alias" "index_signing" {
  name          = "alias/${var.project}-index-signing"
  target_key_id = aws_kms_key.index_signing.key_id
}

data "aws_kms_public_key" "index_signing" {
  key_id = aws_kms_key.index_signing.arn
}
