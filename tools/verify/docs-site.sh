#!/usr/bin/env bash
# The public user guide on GitHub Pages at a domain's apex (decision 0010
# step 1). Read-only: DNS queries and HTTPS requests to the public site; no
# account, no container, nothing changed.
#
# It checks, in this order, and stops at the first failure:
#
#   1. the TXT record _github-pages-challenge-<org>.<domain> exists;
#   2. the apex has exactly GitHub Pages' four A and four AAAA addresses;
#   3. www.<domain> is a CNAME to <org>.github.io and nothing else;
#   4. http:// redirects to https://, and www to the apex;
#   5. the certificate covers the domain;
#   6. / leads to the landing page, /guide/platform/ (the docs site's
#      `redirects` in astro.config.mjs, since docs 0.6.0);
#   7. the landing page in both languages, /guide/platform/ and
#      /es/guide/platform/, is 200, each with
#      <link rel="canonical" href="https://<domain>/...">;
#   8. a missing page is 404 with the guide's 404 page;
#   9. none of those pages holds an /api/ or /docs/ link.
#
# Needs: dig, curl, openssl, grep.
#
#   tools/verify/docs-site.sh orquestador.app government-data
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <domain> <org>" >&2
  exit 2
fi
domain=$1
org=$2
url="https://$domain"
# The page / leads to, and its Spanish twin.
landing=/guide/platform/

# GitHub Pages' apex addresses, from GitHub's "Managing a custom domain".
want_a="185.199.108.153
185.199.109.153
185.199.110.153
185.199.111.153"
want_aaaa="2606:50c0:8000::153
2606:50c0:8001::153
2606:50c0:8002::153
2606:50c0:8003::153"

step=0
ok() { echo "ok   $step. $*"; }
fail() {
  echo "FAIL $step. $*" >&2
  exit 1
}
next() { step=$((step + 1)); }

# Records of one type, sorted, one per line; a CNAME's target without its dot.
records() { dig +short "$1" "$2" | sed 's/\.$//' | sort; }

next
txt=$(records "_github-pages-challenge-$org.$domain" TXT)
[ -n "$txt" ] || fail "no TXT record _github-pages-challenge-$org.$domain"
ok "verification TXT record present"

next
a=$(records "$domain" A)
aaaa=$(records "$domain" AAAA)
[ "$a" = "$want_a" ] || fail "apex A records are not GitHub's: $(echo "$a" | tr '\n' ' ')"
[ "$aaaa" = "$want_aaaa" ] ||
  fail "apex AAAA records are not GitHub's: $(echo "$aaaa" | tr '\n' ' ')"
ok "apex A and AAAA are GitHub Pages' addresses"

next
cname=$(dig +short "www.$domain" CNAME | sed 's/\.$//')
[ "$cname" = "$org.github.io" ] || fail "www.$domain CNAME is '$cname', not $org.github.io"
ok "www.$domain is a CNAME to $org.github.io"

next
# Where a URL's redirect leads (no follow).
location() { curl -sS -o /dev/null -w '%{redirect_url}' --max-time 20 "$1"; }
code() { curl -sS -o /dev/null -w '%{http_code}' --max-time 20 "$1"; }
to=$(location "http://$domain/")
case "$to" in "https://$domain/"*) ;; *) fail "http://$domain/ redirects to '$to'" ;; esac
to=$(location "https://www.$domain/")
case "$to" in "https://$domain/"*) ;; *) fail "https://www.$domain/ redirects to '$to'" ;; esac
ok "http:// redirects to https://, www to the apex"

next
names=$(echo | openssl s_client -connect "$domain:443" -servername "$domain" 2>/dev/null |
  openssl x509 -noout -ext subjectAltName 2>/dev/null || true)
grep -q "DNS:$domain\(,\|$\)" <<<"$names" || fail "the certificate does not name $domain"
ok "the certificate covers $domain"

next
# GitHub Pages has no server-side redirects: / is a page whose meta refresh
# leads to the landing page; curl follows HTTP redirects only.
root=$(curl -fsSL --max-time 20 "$url/")
grep -qi "http-equiv=\"refresh\"[^>]*url=$landing\"" <<<"$root" ||
  fail "/ does not lead to $landing"
ok "/ leads to $landing"

next
pages=""
for path in "$landing" "/es$landing"; do
  [ "$(code "$url$path")" = 200 ] || fail "$path is not 200"
  page=$(curl -fsS --max-time 20 "$url$path")
  grep -q "<link rel=\"canonical\" href=\"$url$path\"" <<<"$page" ||
    fail "$path has no canonical link to $url$path"
  pages+=$page
done
ok "the landing page in both languages is 200 with its canonical link"

next
missing="$url/qs-verify-missing-page/"
[ "$(code "$missing")" = 404 ] || fail "a missing page is not 404"
page=$(curl -sS --max-time 20 "$missing")
grep -q '<title>404 |' <<<"$page" || fail "the 404 is not the guide's"
pages+=$page
ok "a missing page is 404 with the guide's page"

next
if grep -Eo '(href|src|action)="/(api|docs)/[^"]*"|fetch\("/(api|docs)/' <<<"$pages"; then
  fail "a page links to /api/ or /docs/"
fi
ok "no /api/ or /docs/ links"

echo "all checks passed for $domain ($org)"
