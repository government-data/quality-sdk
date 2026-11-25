# The public user guide on GitHub Pages at the domain's apex (decision 0010):
# the apex A and AAAA records with Pages' addresses, www as a CNAME to the
# organisation's github.io name (Pages redirects it to the apex) and the
# organisation's domain verification TXT, which must stay in DNS. No
# wildcard (GitHub calls it an immediate takeover risk) and no CAA (without
# one, Let's Encrypt, Pages' certificate authority, may issue; one added
# later must allow letsencrypt.org).
#
# The hosted zone came with the domain registration and is only read. The
# records were created by the orchestrator's root (knime-orchestador
# infra/aws), which forgets them; the import blocks take them over here
# without recreating them. Apply that root first.

locals {
  site_pages_ipv4 = [
    "185.199.108.153",
    "185.199.109.153",
    "185.199.110.153",
    "185.199.111.153",
  ]
  site_pages_ipv6 = [
    "2606:50c0:8000::153",
    "2606:50c0:8001::153",
    "2606:50c0:8002::153",
    "2606:50c0:8003::153",
  ]
  site_zone_id = data.aws_route53_zone.site.zone_id
}

data "aws_route53_zone" "site" {
  name = var.site_domain
}

resource "aws_route53_record" "site_apex" {
  zone_id = local.site_zone_id
  name    = var.site_domain
  type    = "A"
  ttl     = 3600
  records = local.site_pages_ipv4
}

resource "aws_route53_record" "site_apex_ipv6" {
  zone_id = local.site_zone_id
  name    = var.site_domain
  type    = "AAAA"
  ttl     = 3600
  records = local.site_pages_ipv6
}

resource "aws_route53_record" "site_www" {
  zone_id = local.site_zone_id
  name    = "www.${var.site_domain}"
  type    = "CNAME"
  ttl     = 3600
  records = ["${var.site_github_org}.github.io"]
}

# The name carries the organisation: after a rename GitHub gives a new TXT,
# and changing site_github_org with site_pages_verification replaces this
# record (the new one first, then the old one goes).
resource "aws_route53_record" "site_verification" {
  zone_id = local.site_zone_id
  name    = "_github-pages-challenge-${var.site_github_org}.${var.site_domain}"
  type    = "TXT"
  ttl     = 3600
  records = [var.site_pages_verification]

  lifecycle {
    create_before_destroy = true
  }
}

# Take over the records the orchestrator's root created. The verification
# TXT imported is the one of the organisation's old name (quality-data): the
# plan replaces it with government-data's, so its deletion is explicit here.
import {
  to = aws_route53_record.site_apex
  id = "${local.site_zone_id}_${var.site_domain}_A"
}

import {
  to = aws_route53_record.site_apex_ipv6
  id = "${local.site_zone_id}_${var.site_domain}_AAAA"
}

import {
  to = aws_route53_record.site_www
  id = "${local.site_zone_id}_www.${var.site_domain}_CNAME"
}

import {
  to = aws_route53_record.site_verification
  id = "${local.site_zone_id}__github-pages-challenge-quality-data.${var.site_domain}_TXT"
}
