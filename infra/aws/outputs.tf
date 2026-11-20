output "releases_bucket" {
  description = "The release bucket: index, packages and reports."
  value       = aws_s3_bucket.releases.bucket
}

output "repositories" {
  description = "ECR repository URL per module image."
  value       = { for k, r in aws_ecr_repository.image : k => r.repository_url }
}

output "trust_anchor_arn" {
  description = "Roles Anywhere trust anchor: the agent's `--trust-anchor-arn`."
  value       = aws_rolesanywhere_trust_anchor.installations.arn
}

output "profile_arn" {
  description = "Roles Anywhere profile: the agent's `--profile-arn`."
  value       = aws_rolesanywhere_profile.installations.arn
}

output "installation_role_arn" {
  description = "The role an installation gets: the agent's `--role-arn`."
  value       = aws_iam_role.installation.arn
}

output "publish_role_arn" {
  description = "The role GitHub Actions assumes to publish (secret AWS_PUBLISH_ROLE)."
  value       = aws_iam_role.publish.arn
}

output "index_signing_key" {
  description = "The KMS key that signs the index (alias)."
  value       = aws_kms_alias.index_signing.name
}

output "index_public_key_pem" {
  description = "The index's public key, to pin in the agent."
  value       = data.aws_kms_public_key.index_signing.public_key_pem
}
