#!/usr/bin/env bash
# Verifies IAM Roles Anywhere for installations (decision 0001, step 2)
# against the applied Terraform in infra/aws. Run by a person, never by an
# agent or in CI: it calls the real account.
#
# It issues short-lived test certificates from the installations' CA,
# trades them for installation-role credentials, checks what the role may
# and may not do, revokes one certificate with a CRL and checks the refusal.
# Everything it creates (certificates, the CRL, `qs-verify-` objects) is
# removed on exit, also on failure.
#
#   QD_CA_CERT=~/qd-ca/ca.pem QD_CA_KEY=~/qd-ca/ca.key \
#     tools/verify/roles-anywhere.sh
#
# Environment:
#   QD_CA_CERT          the CA certificate (default ~/qd-ca/ca.pem)
#   QD_CA_KEY           the CA private key, read in place (default ~/qd-ca/ca.key)
#   QD_AWS_PROFILE      the person's profile, for setup, CRL and cleanup
#                       (default orquestador)
#   QD_SIGNING_HELPER   a local aws_signing_helper instead of downloading it
#   QD_CRL_WAIT         seconds to wait for the CRL to take effect (default 600)
#
# Needs: terraform, aws (v2), openssl, jq, curl, sha256sum.

set -euo pipefail

HELPER_VERSION="1.8.6"
HELPER_URL="https://rolesanywhere.amazonaws.com/releases/${HELPER_VERSION}/X86_64/Linux/Amzn2023/aws_signing_helper"
HELPER_SHA256="3553fdc515e5682541a9ebdd37031ec7f461169aa13ccbb6fbd5b7259644113d"

CA_CERT="${QD_CA_CERT:-$HOME/qd-ca/ca.pem}"
CA_KEY="${QD_CA_KEY:-$HOME/qd-ca/ca.key}"
PERSON_PROFILE="${QD_AWS_PROFILE:-orquestador}"
CRL_WAIT="${QD_CRL_WAIT:-600}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

RUN="qs-verify-$(date -u +%Y%m%dT%H%M%SZ)-$(openssl rand -hex 3)"
ID_A="qs-verify-a-$(openssl rand -hex 4)"
ID_B="qs-verify-b-$(openssl rand -hex 4)"
WORK="$(mktemp -d)"
CRL_ID=""
BUCKET=""
REGION=""
FAILED=0
RESULTS=()

person() { aws --profile "$PERSON_PROFILE" --region "$REGION" "$@"; }

# Runs aws as installation $1 through credential-process, with none of the
# person's credentials in the environment.
as_inst() {
  local who="$1"
  shift
  env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN \
    -u AWS_PROFILE -u AWS_DEFAULT_PROFILE \
    AWS_CONFIG_FILE="$WORK/aws-config" AWS_SHARED_CREDENTIALS_FILE=/dev/null \
    aws --profile "inst-$who" --region "$REGION" "$@"
}

record() { # record PASS|FAIL name [detail]
  local line="$1  $2${3:+ ($3)}"
  echo "$line"
  RESULTS+=("$line")
  if [[ "$1" == FAIL ]]; then FAILED=1; fi
}

denied() { grep -qE 'AccessDenied|not authorized|Forbidden|\(403\)' "$1"; }
first_error() { grep -m1 -E 'error|Error' "$1" | cut -c1-200 || true; }

expect_ok() { # expect_ok name cmd...
  local name="$1"
  shift
  if "$@" >"$WORK/out" 2>"$WORK/err"; then
    record PASS "$name"
  else
    record FAIL "$name" "$(first_error "$WORK/err")"
  fi
}

expect_denied() { # expect_denied name cmd...
  local name="$1"
  shift
  if "$@" >"$WORK/out" 2>"$WORK/err"; then
    record FAIL "$name" "allowed, expected AccessDenied"
  elif denied "$WORK/err"; then
    record PASS "$name"
  else
    record FAIL "$name" "$(first_error "$WORK/err")"
  fi
}

# Deletes every version and delete marker under a prefix (the bucket is
# versioned, so a plain delete would leave the object behind).
# shellcheck disable=SC2329 # called from cleanup, the EXIT trap
purge_prefix() {
  local prefix="$1" versions
  versions="$(person s3api list-object-versions --bucket "$BUCKET" --prefix "$prefix" \
    --output json)" || return 1
  jq -r '((.Versions // []) + (.DeleteMarkers // []))[] | [.Key, .VersionId] | @tsv' \
    <<<"$versions" |
    while IFS=$'\t' read -r key version; do
      person s3api delete-object --bucket "$BUCKET" --key "$key" \
        --version-id "$version" >/dev/null || exit 1
    done
}

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local rc=$? left=0
  set +e
  if [[ -n "$CRL_ID" ]]; then
    person rolesanywhere delete-crl --crl-id "$CRL_ID" >/dev/null ||
      { echo "WARN: delete the CRL $CRL_ID by hand"; left=1; }
  fi
  if [[ -n "$BUCKET" && -n "$REGION" ]]; then
    for p in "index/$RUN" "reports/$ID_A/" "reports/$ID_B/"; do
      purge_prefix "$p" || { echo "WARN: remove s3://$BUCKET/$p* by hand"; left=1; }
    done
  fi
  rm -rf "$WORK"
  if ((left)); then rc=1; fi
  exit "$rc"
}
trap cleanup EXIT

die() {
  echo "ERROR: $*" >&2
  exit 1
}

# --- Setup ----------------------------------------------------------------

for tool in terraform aws openssl jq curl sha256sum; do
  command -v "$tool" >/dev/null || die "$tool is not installed"
done
[[ -r "$CA_CERT" ]] || die "CA certificate not readable: $CA_CERT (set QD_CA_CERT)"
[[ -r "$CA_KEY" ]] || die "CA key not readable: $CA_KEY (set QD_CA_KEY)"

tf() { terraform -chdir="$REPO_ROOT/infra/aws" output -raw "$1"; }
TRUST_ANCHOR="$(tf trust_anchor_arn)"
REGION="$(cut -d: -f4 <<<"$TRUST_ANCHOR")"
PROFILE_ARN="$(tf profile_arn)"
ROLE_ARN="$(tf installation_role_arn)"
BUCKET="$(tf releases_bucket)"
mapfile -t REPOS < <(terraform -chdir="$REPO_ROOT/infra/aws" output -json repositories |
  jq -r '.[] | sub("^[^/]+/"; "")')
ROLE_NAME="${ROLE_ARN##*/}"
((${#REPOS[@]} > 0)) || die "no ECR repositories in the Terraform outputs"

person sts get-caller-identity >/dev/null ||
  die "the AWS profile $PERSON_PROFILE has no valid credentials (aws sso login?)"

HELPER="$WORK/aws_signing_helper"
if [[ -n "${QD_SIGNING_HELPER:-}" ]]; then
  [[ -x "$QD_SIGNING_HELPER" ]] || die "QD_SIGNING_HELPER is not executable"
  cp "$QD_SIGNING_HELPER" "$HELPER"
  echo "Using the local aws_signing_helper, sha256 $(sha256sum "$HELPER" | cut -d' ' -f1)"
else
  [[ "$(uname -sm)" == "Linux x86_64" ]] ||
    die "no pinned helper for $(uname -sm): download it from the IAM Roles Anywhere" \
      "user guide (credential helper page), check its SHA-256 there, set QD_SIGNING_HELPER"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$HELPER" "$HELPER_URL"
  echo "$HELPER_SHA256  $HELPER" | sha256sum -c --quiet - ||
    die "aws_signing_helper $HELPER_VERSION does not match the pinned SHA-256"
  chmod +x "$HELPER"
fi
HELPER_REPORTED="$("$HELPER" version 2>/dev/null || echo unknown)"

# --- Test certificates ----------------------------------------------------

cat >"$WORK/leaf.ext" <<'EOF'
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = clientAuth
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid, issuer
EOF

issue() { # issue name ou cn: a 1-day EC P-256 certificate signed by the CA
  local name="$1" ou="$2" cn="$3"
  openssl ecparam -name prime256v1 -genkey -noout -out "$WORK/$name.key" 2>/dev/null
  openssl req -new -key "$WORK/$name.key" -subj "/OU=$ou/CN=$cn" -out "$WORK/$name.csr"
  openssl x509 -req -in "$WORK/$name.csr" -CA "$CA_CERT" -CAkey "$CA_KEY" \
    -set_serial "0x$(openssl rand -hex 16)" -days 1 -sha256 \
    -extfile "$WORK/leaf.ext" -out "$WORK/$name.pem"
  openssl verify -CAfile "$CA_CERT" "$WORK/$name.pem" >/dev/null
  cat >>"$WORK/aws-config" <<EOF
[profile inst-$name]
credential_process = $HELPER credential-process --certificate $WORK/$name.pem --private-key $WORK/$name.key --trust-anchor-arn $TRUST_ANCHOR --profile-arn $PROFILE_ARN --role-arn $ROLE_ARN --region $REGION
EOF
}

: >"$WORK/aws-config"
issue a installation "$ID_A"
issue b installation "$ID_B"
issue other qs-verify-other "qs-verify-o-$(openssl rand -hex 4)"

# create_session who: CreateSession through the helper; the credentials go
# to a file in the work dir and are never printed.
create_session() {
  "$HELPER" credential-process --certificate "$WORK/$1.pem" --private-key "$WORK/$1.key" \
    --trust-anchor-arn "$TRUST_ANCHOR" --profile-arn "$PROFILE_ARN" \
    --role-arn "$ROLE_ARN" --region "$REGION" >"$WORK/session-$1.json"
}

echo "Run $RUN, installations $ID_A and $ID_B, region $REGION"
echo

# --- Credentials ----------------------------------------------------------

expect_ok "A: CreateSession with OU=installation" create_session a
if [[ -s "$WORK/session-a.json" ]]; then
  expires="$(jq -r .Expiration "$WORK/session-a.json")"
  left=$(($(date -d "$expires" +%s) - $(date +%s)))
  if ((left > 0 && left <= 3660)); then
    record PASS "A: session lasts at most 1 h" "${left}s"
  else
    record FAIL "A: session lasts at most 1 h" "${left}s"
  fi
fi
expect_denied "Other OU is refused at CreateSession" create_session other

arn="$(as_inst a sts get-caller-identity --query Arn --output text 2>"$WORK/err" || true)"
if [[ "$arn" == arn:aws:sts::*:assumed-role/"$ROLE_NAME"/* ]]; then
  record PASS "A: caller is the installation role"
else
  record FAIL "A: caller is the installation role" "${arn:-$(first_error "$WORK/err")}"
fi

# --- S3 -------------------------------------------------------------------

# The role has GetObject only, no ListBucket: a missing key also answers
# AccessDenied. So read an index object the person confirms exists, or put a
# test one as the person when index/ is empty.
index_key="$(person s3api list-objects-v2 --bucket "$BUCKET" --prefix index/ \
  --max-keys 1 --query 'Contents[0].Key' --output text)"
if [[ -z "$index_key" || "$index_key" == None ]]; then
  index_key="index/$RUN.json"
  echo '{"qs_verify": true}' >"$WORK/index-test.json"
  person s3api put-object --bucket "$BUCKET" --key "$index_key" \
    --body "$WORK/index-test.json" >/dev/null
fi
expect_ok "A: reads $index_key" as_inst a s3api get-object --bucket "$BUCKET" \
  --key "$index_key" "$WORK/index-read"
expect_denied "A: cannot list the bucket" as_inst a s3api list-objects-v2 \
  --bucket "$BUCKET" --prefix index/ --max-keys 1
expect_denied "A: cannot write index/" as_inst a s3api put-object --bucket "$BUCKET" \
  --key "index/$RUN-denied.json" --body "$WORK/leaf.ext"

echo '{"qs_verify": true}' >"$WORK/report.json"
expect_ok "A: writes reports/<A>/" as_inst a s3api put-object --bucket "$BUCKET" \
  --key "reports/$ID_A/$RUN.json" --body "$WORK/report.json"
expect_denied "A: cannot write reports/<B>/" as_inst a s3api put-object \
  --bucket "$BUCKET" --key "reports/$ID_B/$RUN.json" --body "$WORK/report.json"
expect_denied "A: cannot read reports, not even its own" as_inst a s3api get-object \
  --bucket "$BUCKET" --key "reports/$ID_A/$RUN.json" "$WORK/report-read"

# --- ECR ------------------------------------------------------------------

# The token itself is never printed: only how many entries came back.
expect_ok "A: ECR GetAuthorizationToken" as_inst a ecr get-authorization-token \
  --query 'length(authorizationData)' --output text
for repo in "${REPOS[@]}"; do
  # An absent tag answers 200 with an ImageNotFound failure when pull is allowed.
  expect_ok "A: may pull from $repo" as_inst a ecr batch-get-image \
    --repository-name "$repo" --image-ids imageTag="$RUN"
done
expect_denied "A: cannot push to ${REPOS[0]}" as_inst a ecr initiate-layer-upload \
  --repository-name "${REPOS[0]}"

# --- Revocation -----------------------------------------------------------

cat >"$WORK/ca.cnf" <<EOF
[ca]
default_ca = qd
[qd]
database = $WORK/index.txt
crlnumber = $WORK/crlnumber
default_md = sha256
default_crl_days = 1
EOF
: >"$WORK/index.txt"
echo 01 >"$WORK/crlnumber"
openssl ca -config "$WORK/ca.cnf" -cert "$CA_CERT" -keyfile "$CA_KEY" \
  -revoke "$WORK/a.pem" 2>/dev/null
openssl ca -config "$WORK/ca.cnf" -cert "$CA_CERT" -keyfile "$CA_KEY" \
  -gencrl -out "$WORK/revoked.crl" 2>/dev/null
serial_a="$(openssl x509 -in "$WORK/a.pem" -noout -serial | cut -d= -f2)"
openssl crl -in "$WORK/revoked.crl" -noout -text | grep -qi "$serial_a" ||
  die "the CRL does not list A's serial"

CRL_ID="$(person rolesanywhere import-crl --name "$RUN" --enabled \
  --trust-anchor-arn "$TRUST_ANCHOR" --crl-data "fileb://$WORK/revoked.crl" \
  --query crl.crlId --output text)"
enabled="$(person rolesanywhere get-crl --crl-id "$CRL_ID" --query crl.enabled --output text)"
[[ "$enabled" == True ]] || person rolesanywhere enable-crl --crl-id "$CRL_ID" >/dev/null
echo "Imported CRL $CRL_ID with A's serial; waiting up to ${CRL_WAIT}s for the refusal"

waited=0
revoked=0
while ((waited <= CRL_WAIT)); do
  if ! create_session a 2>"$WORK/err"; then
    revoked=1
    break
  fi
  sleep 20
  waited=$((waited + 20))
done
if ((revoked)) && denied "$WORK/err"; then
  record PASS "A: refused at CreateSession after the CRL" "after ~${waited}s"
elif ((revoked)); then
  record FAIL "A: refused at CreateSession after the CRL" "$(first_error "$WORK/err")"
else
  record FAIL "A: refused at CreateSession after the CRL" "still allowed after ${CRL_WAIT}s"
fi
expect_ok "B: CreateSession still works" create_session b
expect_ok "B: writes reports/<B>/" as_inst b s3api put-object --bucket "$BUCKET" \
  --key "reports/$ID_B/$RUN.json" --body "$WORK/report.json"

# --- Summary --------------------------------------------------------------

cat <<EOF

----- paste below into decision 0001, Verification -----
date: $(date -u +%Y-%m-%d)
aws_signing_helper: $HELPER_REPORTED
$(aws --version 2>&1)
$(openssl version)
$(printf '%s\n' "${RESULTS[@]}")
result: $( ((FAILED)) && echo FAIL || echo PASS)
---------------------------------------------------------
EOF
exit "$FAILED"
