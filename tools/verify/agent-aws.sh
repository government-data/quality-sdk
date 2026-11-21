#!/usr/bin/env bash
# Checks the agent's AWS adapter end to end (decision 0003, step 6) against
# the applied Terraform in infra/aws. Run by a person, never by an agent or
# in CI: it calls the real account.
#
# It issues a short-lived test certificate from the installations' CA, lays
# out an installation under a temporary root (agent.toml, the certificate,
# the index key pinned from Terraform) and runs `quality-data-agent check`:
# Roles Anywhere credentials, the signed index read and verified, the
# registry's credential, one report of kind `check`. Then it runs the check
# again with another pinned key and expects the index to be refused.
#
# When the channel has no index yet, it signs a test index with the KMS key
# (as publishing will: the SHA-256 of the index, MessageType=DIGEST), with
# no versions, and removes exactly what it put on exit. An existing index is
# only read. Its report objects are removed on exit, also on failure.
#
#   QD_CA_CERT=~/qd-ca/ca.pem QD_CA_KEY=~/qd-ca/ca.key \
#     QD_AGENT=target/release/quality-data-agent tools/verify/agent-aws.sh
#
# Environment:
#   QD_CA_CERT          the CA certificate (default ~/qd-ca/ca.pem)
#   QD_CA_KEY           the CA private key, read in place (default ~/qd-ca/ca.key)
#   QD_AGENT            the built agent (default
#                       ${CARGO_TARGET_DIR:-target}/release/quality-data-agent)
#   QD_AWS_PROFILE      the person's profile, for setup and cleanup
#                       (default orquestador)
#   QD_SIGNING_HELPER   a local aws_signing_helper instead of downloading it
#   QD_CHANNEL          stable or patch (default stable)
#
# Needs: terraform, aws (v2), openssl, jq, curl, sha256sum.

set -euo pipefail

HELPER_VERSION="1.8.6"
HELPER_URL="https://rolesanywhere.amazonaws.com/releases/${HELPER_VERSION}/X86_64/Linux/Amzn2023/aws_signing_helper"
HELPER_SHA256="3553fdc515e5682541a9ebdd37031ec7f461169aa13ccbb6fbd5b7259644113d"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CA_CERT="${QD_CA_CERT:-$HOME/qd-ca/ca.pem}"
CA_KEY="${QD_CA_KEY:-$HOME/qd-ca/ca.key}"
AGENT="${QD_AGENT:-${CARGO_TARGET_DIR:-$REPO_ROOT/target}/release/quality-data-agent}"
PERSON_PROFILE="${QD_AWS_PROFILE:-orquestador}"
CHANNEL="${QD_CHANNEL:-stable}"

ID="qs-verify-agent-$(openssl rand -hex 4)"
WORK="$(mktemp -d)"
ROOT="$WORK/root"
BUCKET=""
REGION=""
PUT_VERSIONS=() # "key<TAB>version id" of every test index object put
FAILED=0
RESULTS=()

person() { aws --profile "$PERSON_PROFILE" --region "$REGION" "$@"; }

record() { # record PASS|FAIL name [detail]
  local line="$1  $2${3:+ ($3)}"
  echo "$line"
  RESULTS+=("$line")
  if [[ "$1" == FAIL ]]; then FAILED=1; fi
}

die() {
  echo "ERROR: $*" >&2
  exit 1
}

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local rc=$? left=0 entry key version versions
  set +e
  for entry in "${PUT_VERSIONS[@]}"; do
    IFS=$'\t' read -r key version <<<"$entry"
    person s3api delete-object --bucket "$BUCKET" --key "$key" --version-id "$version" \
      >/dev/null || { echo "WARN: remove s3://$BUCKET/$key ($version) by hand"; left=1; }
  done
  if [[ -n "$BUCKET" && -n "$REGION" ]]; then
    if versions="$(person s3api list-object-versions --bucket "$BUCKET" \
      --prefix "reports/$ID/" --output json)"; then
      jq -r '((.Versions // []) + (.DeleteMarkers // []))[] | [.Key, .VersionId] | @tsv' \
        <<<"$versions" |
        while IFS=$'\t' read -r key version; do
          person s3api delete-object --bucket "$BUCKET" --key "$key" \
            --version-id "$version" >/dev/null || echo "WARN: remove s3://$BUCKET/$key by hand"
        done
    else
      echo "WARN: remove s3://$BUCKET/reports/$ID/ by hand"
      left=1
    fi
  fi
  rm -rf "$WORK"
  if ((left)); then rc=1; fi
  exit "$rc"
}
trap cleanup EXIT

# --- Setup ----------------------------------------------------------------

for tool in terraform aws openssl jq curl sha256sum; do
  command -v "$tool" >/dev/null || die "$tool is not installed"
done
[[ -r "$CA_CERT" ]] || die "CA certificate not readable: $CA_CERT (set QD_CA_CERT)"
[[ -r "$CA_KEY" ]] || die "CA key not readable: $CA_KEY (set QD_CA_KEY)"
[[ -x "$AGENT" ]] || die "no agent at $AGENT: build it with" \
  "'~/.local/bin/capped cargo build --release -j 2 -p agent' or set QD_AGENT"
[[ "$CHANNEL" == stable || "$CHANNEL" == patch ]] || die "QD_CHANNEL is stable or patch"

tf() { terraform -chdir="$REPO_ROOT/infra/aws" output -raw "$1"; }
TRUST_ANCHOR="$(tf trust_anchor_arn)"
REGION="$(cut -d: -f4 <<<"$TRUST_ANCHOR")"
PROFILE_ARN="$(tf profile_arn)"
ROLE_ARN="$(tf installation_role_arn)"
BUCKET="$(tf releases_bucket)"
SIGNING_KEY="$(tf index_signing_key)"

person sts get-caller-identity >/dev/null ||
  die "the AWS profile $PERSON_PROFILE has no valid credentials (aws sso login?)"

HELPER="$WORK/aws_signing_helper"
if [[ -n "${QD_SIGNING_HELPER:-}" ]]; then
  [[ -x "$QD_SIGNING_HELPER" ]] || die "QD_SIGNING_HELPER is not executable"
  cp "$QD_SIGNING_HELPER" "$HELPER"
else
  [[ "$(uname -sm)" == "Linux x86_64" ]] ||
    die "no pinned helper for $(uname -sm): set QD_SIGNING_HELPER"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$HELPER" "$HELPER_URL"
  echo "$HELPER_SHA256  $HELPER" | sha256sum -c --quiet - ||
    die "aws_signing_helper $HELPER_VERSION does not match the pinned SHA-256"
  chmod +x "$HELPER"
fi

# --- The installation under $ROOT -----------------------------------------

CONF="$ROOT/etc/quality-data"
mkdir -p "$CONF/enrolment"
cat >"$WORK/leaf.ext" <<'EOF'
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = clientAuth
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid, issuer
EOF
openssl ecparam -name prime256v1 -genkey -noout -out "$CONF/enrolment/private-key.pem" 2>/dev/null
openssl req -new -key "$CONF/enrolment/private-key.pem" -subj "/OU=installation/CN=$ID" \
  -out "$WORK/leaf.csr"
openssl x509 -req -in "$WORK/leaf.csr" -CA "$CA_CERT" -CAkey "$CA_KEY" \
  -set_serial "0x$(openssl rand -hex 16)" -days 1 -sha256 -extfile "$WORK/leaf.ext" \
  -out "$CONF/enrolment/certificate.pem" 2>/dev/null
tf index_public_key_pem >"$CONF/index-key.pem"
cat >"$CONF/agent.toml" <<EOF
installation = "$ID"
channel = "$CHANNEL"

[aws]
region = "$REGION"
bucket = "$BUCKET"
trust_anchor_arn = "$TRUST_ANCHOR"
profile_arn = "$PROFILE_ARN"
role_arn = "$ROLE_ARN"
signing_helper = "$HELPER"
EOF

# --- A test index, only when the channel has none --------------------------

INDEX_KEY="index/$CHANNEL.json"
if person s3api head-object --bucket "$BUCKET" --key "$INDEX_KEY" >/dev/null 2>&1; then
  echo "Reading the published $INDEX_KEY (left as it is)"
else
  echo "No $INDEX_KEY: signing a test index with $SIGNING_KEY (removed on exit)"
  jq -n --arg c "$CHANNEL" --arg t "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{format: 1, channel: $c, sequence: 1, published_at: $t, versions: []}' >"$WORK/index.json"
  openssl dgst -sha256 -binary "$WORK/index.json" >"$WORK/index.digest"
  person kms sign --key-id "$SIGNING_KEY" --message "fileb://$WORK/index.digest" \
    --message-type DIGEST --signing-algorithm ECDSA_SHA_256 \
    --query Signature --output text >"$WORK/index.json.sig"
  for file in index.json index.json.sig; do
    key="index/$CHANNEL.${file#index.}"
    version="$(person s3api put-object --bucket "$BUCKET" --key "$key" \
      --body "$WORK/$file" --query VersionId --output text)"
    PUT_VERSIONS+=("$key"$'\t'"$version")
  done
fi

# --- The agent's check ------------------------------------------------------

echo
if "$AGENT" --root "$ROOT" check | tee "$WORK/check.out"; then
  record PASS "agent check against the account"
else
  record FAIL "agent check against the account"
fi
report="$(awk '$1 == "report" {print $2}' "$WORK/check.out")"
if [[ -n "$report" ]] && person s3api head-object --bucket "$BUCKET" --key "$report" >/dev/null; then
  record PASS "its report is at s3://$BUCKET/$report"
else
  record FAIL "its report is in the bucket" "${report:-no report key}"
fi

# Another pinned key: the same index must not verify.
openssl ecparam -name prime256v1 -genkey -noout 2>/dev/null |
  openssl ec -pubout 2>/dev/null >"$CONF/index-key.pem"
if "$AGENT" --root "$ROOT" check >"$WORK/other.out" 2>&1; then
  record FAIL "an index signed by another key is refused" "it was accepted"
elif grep -q "signature does not verify" "$WORK/other.out"; then
  record PASS "an index signed by another key is refused"
else
  record FAIL "an index signed by another key is refused" "$(tail -1 "$WORK/other.out")"
fi

# --- Summary --------------------------------------------------------------

cat <<EOF

----- paste below into decision 0003, Verification -----
date: $(date -u +%Y-%m-%d)
agent: $("$AGENT" --version)
aws_signing_helper: $("$HELPER" version 2>/dev/null || echo unknown)
$(aws --version 2>&1)
index: $([[ ${#PUT_VERSIONS[@]} -gt 0 ]] && echo "test index signed by KMS" || echo "published $INDEX_KEY")
$(printf '%s\n' "${RESULTS[@]}")
result: $( ((FAILED)) && echo FAIL || echo PASS)
---------------------------------------------------------
EOF
exit "$FAILED"
