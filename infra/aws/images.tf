# One ECR repository per module image (decision 0001). Tags are immutable: a
# published version is never rewritten, and installations pull by digest.

locals {
  repositories = toset(flatten([
    for module, images in var.modules : [for image in images : "${module}/${image}"]
  ]))
}

resource "aws_ecr_repository" "image" {
  for_each             = local.repositories
  name                 = "${var.project}/${each.key}"
  image_tag_mutability = "IMMUTABLE"
  image_scanning_configuration {
    scan_on_push = true
  }
  encryption_configuration {
    encryption_type = "AES256"
  }
}

resource "aws_ecr_lifecycle_policy" "image" {
  for_each   = aws_ecr_repository.image
  repository = each.value.name
  policy = jsonencode({
    rules = [
      {
        rulePriority = 1
        description  = "Untagged images go after a day"
        selection = {
          tagStatus   = "untagged"
          countType   = "sinceImagePushed"
          countUnit   = "days"
          countNumber = 1
        }
        action = { type = "expire" }
      },
      {
        rulePriority = 2
        description  = "Keep the latest versions"
        selection = {
          tagStatus      = "tagged"
          tagPatternList = ["*"]
          countType      = "imageCountMoreThan"
          countNumber    = var.versions_per_repository
        }
        action = { type = "expire" }
      },
    ]
  })
}
