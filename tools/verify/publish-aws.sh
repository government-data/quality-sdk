#!/usr/bin/env bash
# Checks the quality-sdk CLI end to end (decision 0001, step 6) against the
# applied Terraform in infra/aws, with the agent on the other side. Run by a
# person, never by an agent or in CI: it calls the real account.
#
#   1. `quality-sdk enrol` lays out a test installation's key and request,
#      `quality-sdk sign-enrolment` signs it for one day with the CA.
#   2. `quality-sdk publish` publishes a test platform version (one module,
#      `qs-verify`, with a tiny package) to a channel that has no index yet,
#      pinned to the Terraform's index key; a second version is published
#      with the key read from KMS (sequence 2); the same version again is
#      refused.
#   3. `quality-data-agent check --download` gets the offer (the test
#      installation is the version's only pilot), downloads the package and
#      checks its SHA-256.
#   4. `quality-sdk revoke` imports a test CRL (named after the run) with the
#      test certificate, and the agent is refused within QD_CRL_WAIT seconds.
#
# Everything it creates (the S3 object versions under index/<channel>.json,
# packages/qs-verify/ and reports/<id>/, the test CRL) is removed on exit,
# also on failure. A channel that already has an index is refused: a test
# version is never added to a published channel.
#
#   QD_CA_CERT=~/qd-ca/ca.pem QD_CA_KEY=~/qd-ca/ca.key tools/verify/publish-aws.sh
#
# Environment:
#   QD_CA_CERT          the CA certificate (default ~/qd-ca/ca.pem)
#   QD_CA_KEY           the CA private key, read in place (default ~/qd-ca/ca.key)
#   QD_SDK              the built CLI (default
#                       ${CARGO_TARGET_DIR:-target}/release/quality-sdk)
#   QD_AGENT            the built agent (default
#                       ${CARGO_TARGET_DIR:-target}/release/quality-data-agent)
#   QD_AWS_PROFILE      the person's profile: publishing, revoking, cleanup
#                       (default orquestador)
#   QD_SIGNING_HELPER   a local aws_signing_helper instead of downloading it
#   QD_CHANNEL          the channel to publish on, without an index (default patch)
#   QD_IMAGE            an image `<repo>@sha256:<digest>` for the test module
#                       (default: the first repository with digest 0…0; the
#                       check pulls no image)
#   QD_CRL_WAIT         seconds to wait for the CRL to take effect (default 600)
#   QD_SKIP_REVOKE      1 to skip step 4
#
# Needs: terraform, aws (v2), openssl, jq, curl, sha256sum, tar.

set -euo pipefail

HELPER_VERSION="1.8.6"
HELPER_URL="https://rolesanywhere.amazonaws.com/releases/${HELPER_VERSION}/X86_64/Linux/Amzn2023/aws_signing_helper"
HELPER_SHA256="3553fdc515e5682541a9ebdd37031ec7f461169aa13ccbb6fbd5b7259644113d"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CA_CERT="${QD_CA_CERT:-$HOME/qd-ca/ca.pem}"
CA_KEY="${QD_CA_KEY:-$HOME/qd-ca/ca.key}"
TARGET="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/release"
SDK="${QD_SDK:-$TARGET/quality-sdk}"
AGENT="${QD_AGENT:-$TARGET/quality-data-agent}"
PERSON_PROFILE="${QD_AWS_PROFILE:-orquestador}"
CHANNEL="${QD_CHANNEL:-patch}"
CRL_WAIT="${QD_CRL_WAIT:-600}"

RUN="qs-verify-$(date -u +%Y%m%dT%H%M%SZ)-$(openssl rand -hex 3)"
ID="qs-verify-publish-$(openssl rand -hex 4)"
WORK="$(mktemp -d)"
ROOT="$WORK/root"
CONF="$ROOT/etc/quality-data"
BUCKET=""
REGION=""
CRL_NAME=""
SNAPSHOT=0 # 1 once what was there before the run is listed in before.tsv
FAILED=0
RESULTS=()

person() { aws --profile "$PERSON_PROFILE" --region "$REGION" "$@"; }

# Runs the CLI with the person's credentials in its environment only.
sdk_as_person() {
  (
    eval "$(aws configure export-credentials --profile "$PERSON_PROFILE" --format env)"
    exec "$SDK" "$@"
  )
}

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

# Every version (and delete marker) of the objects under a prefix, as
# "key<TAB>version id", into a file.
versions_under() { # versions_under prefix file
  person s3api list-object-versions --bucket "$BUCKET" --prefix "$1" --output json |
    jq -r '((.Versions // []) + (.DeleteMarkers // []))[] | [.Key, .VersionId] | @tsv' >"$2"
}

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local rc=$? left=0 prefix key version id
  set +e
  if ((SNAPSHOT)); then
    for prefix in "index/$CHANNEL.json" "packages/qs-verify/" "reports/$ID/"; do
      if versions_under "$prefix" "$WORK/now.tsv"; then
        # Only what was not there before this run.
        grep -vxF -f "$WORK/before.tsv" "$WORK/now.tsv" |
          while IFS=$'\t' read -r key version; do
            [[ "$key" == "$prefix"* ]] || continue
            person s3api delete-object --bucket "$BUCKET" --key "$key" \
              --version-id "$version" >/dev/null || echo "WARN: remove s3://$BUCKET/$key by hand"
          done
      else
        echo "WARN: remove what this run left under s3://$BUCKET/$prefix by hand"
        left=1
      fi
    done
  fi
  if [[ -n "$CRL_NAME" ]]; then
    person rolesanywhere list-crls --output json |
      jq -r --arg n "$CRL_NAME" --arg t "$TRUST_ANCHOR" \
        '.crls[] | select(.name == $n and .trustAnchorArn == $t) | .crlId' >"$WORK/crls" ||
      { echo "WARN: delete the CRL $CRL_NAME by hand"; left=1; }
    while read -r id; do
      person rolesanywhere delete-crl --crl-id "$id" >/dev/null ||
        { echo "WARN: delete the CRL $id ($CRL_NAME) by hand"; left=1; }
    done <"$WORK/crls"
  fi
  rm -rf "$WORK"
  if ((left)); then rc=1; fi
  exit "$rc"
}
trap cleanup EXIT

# --- Setup ----------------------------------------------------------------

for tool in terraform aws openssl jq curl sha256sum tar; do
  command -v "$tool" >/dev/null || die "$tool is not installed"
done
[[ -r "$CA_CERT" ]] || die "CA certificate not readable: $CA_CERT (set QD_CA_CERT)"
[[ -r "$CA_KEY" ]] || die "CA key not readable: $CA_KEY (set QD_CA_KEY)"
for bin in "$SDK" "$AGENT"; do
  [[ -x "$bin" ]] || die "no binary at $bin: build them with '~/.local/bin/capped cargo build" \
    "--release -j 2 -p sdk-cli' and '-p agent', or set QD_SDK and QD_AGENT"
done
[[ "$CHANNEL" == stable || "$CHANNEL" == patch ]] || die "QD_CHANNEL is stable or patch"

tf() { terraform -chdir="$REPO_ROOT/infra/aws" output -raw "$1"; }
TRUST_ANCHOR="$(tf trust_anchor_arn)"
REGION="$(cut -d: -f4 <<<"$TRUST_ANCHOR")"
PROFILE_ARN="$(tf profile_arn)"
ROLE_ARN="$(tf installation_role_arn)"
SIGNING_KEY="$(tf index_signing_key)"
IMAGE="${QD_IMAGE:-$(terraform -chdir="$REPO_ROOT/infra/aws" output -json repositories |
  jq -r 'to_entries[0].value')@sha256:$(printf '0%.0s' {1..64})}"
BUCKET="$(tf releases_bucket)"

person sts get-caller-identity >/dev/null ||
  die "the AWS profile $PERSON_PROFILE has no valid credentials (aws sso login?)"

INDEX_KEY="index/$CHANNEL.json"
if person s3api head-object --bucket "$BUCKET" --key "$INDEX_KEY" >/dev/null 2>&1; then
  die "$INDEX_KEY exists: this check publishes only on a channel without an index" \
    "(set QD_CHANNEL to one)"
fi
: >"$WORK/before.tsv"
for prefix in "index/$CHANNEL.json" "packages/qs-verify/" "reports/$ID/"; do
  versions_under "$prefix" "$WORK/one.tsv"
  cat "$WORK/one.tsv" >>"$WORK/before.tsv"
done
SNAPSHOT=1

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

# --- 1. Enrol and sign ------------------------------------------------------

if "$SDK" enrol --installation "$ID" --root "$ROOT" >"$WORK/request.pem" 2>"$WORK/enrol.err" &&
  [[ "$(stat -c %a "$CONF/enrolment/private-key.pem")" == 600 ]]; then
  record PASS "enrol: key (0600) and request for $ID"
else
  record FAIL "enrol" "$(tail -1 "$WORK/enrol.err")"
  exit 1
fi
if "$SDK" sign-enrolment --ca-cert "$CA_CERT" --ca-key "$CA_KEY" --days 1 \
  --out "$CONF/enrolment/certificate.pem" "$CONF/enrolment/request.csr" 2>"$WORK/sign.err" &&
  openssl verify -CAfile "$CA_CERT" "$CONF/enrolment/certificate.pem" >/dev/null; then
  record PASS "sign-enrolment: a certificate OpenSSL verifies under the CA"
else
  record FAIL "sign-enrolment" "$(tail -1 "$WORK/sign.err")"
  exit 1
fi
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

# --- 2. Publish -------------------------------------------------------------

publication() { # publication version: a version file for the test module
  mkdir -p "$WORK/$1"
  echo "quality-sdk publish check $RUN $1" >"$WORK/$1/README"
  tar -czf "$WORK/$1/qs-verify.tar.gz" -C "$WORK/$1" README
  cat >"$WORK/$1/version.toml" <<EOF
version = "$1"
upgrade_from = "<$1"
notes = "A test version of tools/verify/publish-aws.sh ($RUN)."

[rollout]
percent = 0
pilots = ["$ID"]

[modules.qs-verify]
version = "$1"
package = "qs-verify.tar.gz"

[modules.qs-verify.images]
test = "$IMAGE"
EOF
}
V1="0.0.1-${RUN#qs-verify-}"
V1="${V1//[^0-9A-Za-z.-]/-}"
V2="0.0.2-${RUN#qs-verify-}"
V2="${V2//[^0-9A-Za-z.-]/-}"
publication "$V1"
publication "$V2"
common=(--channel "$CHANNEL" --bucket "$BUCKET" --region "$REGION" --kms-key "$SIGNING_KEY")

if sdk_as_person publish "${common[@]}" --index-key "$CONF/index-key.pem" \
  "$WORK/$V1/version.toml" 2>"$WORK/p1.err" && grep -q "sequence 1," "$WORK/p1.err"; then
  record PASS "publish $V1: sequence 1, KMS signature verified with the pinned key"
else
  record FAIL "publish $V1" "$(tail -1 "$WORK/p1.err")"
  exit 1
fi
if sdk_as_person publish "${common[@]}" "$WORK/$V2/version.toml" 2>"$WORK/p2.err" &&
  grep -q "sequence 2," "$WORK/p2.err"; then
  record PASS "publish $V2: sequence 2, verified with KMS's public key"
else
  record FAIL "publish $V2" "$(tail -1 "$WORK/p2.err")"
fi
if sdk_as_person publish "${common[@]}" "$WORK/$V2/version.toml" 2>"$WORK/p3.err"; then
  record FAIL "the same version again is refused" "it was published"
elif grep -q "already in" "$WORK/p3.err"; then
  record PASS "the same version again is refused"
else
  record FAIL "the same version again is refused" "$(tail -1 "$WORK/p3.err")"
fi

# --- 3. The agent -------------------------------------------------------------

echo
if "$AGENT" --root "$ROOT" check --download | tee "$WORK/check.out"; then
  record PASS "agent check --download against the account"
else
  record FAIL "agent check --download against the account"
fi
if grep -q "^offer .* $V2 from 0.0.0: qs-verify" "$WORK/check.out"; then
  record PASS "the pilot is offered $V2"
else
  record FAIL "the pilot is offered $V2" "$(grep '^offer' "$WORK/check.out" || true)"
fi
if grep -q "^package .* qs-verify packages/qs-verify/$V2.tar.gz, SHA-256 verified" "$WORK/check.out" &&
  cmp -s "$WORK/$V2/qs-verify.tar.gz" "$ROOT/var/lib/quality-data/updater/packages/qs-verify/$V2.tar.gz"; then
  record PASS "its package is downloaded and its SHA-256 verified"
else
  record FAIL "its package is downloaded and its SHA-256 verified"
fi

# --- 4. Revoke ----------------------------------------------------------------

if [[ "${QD_SKIP_REVOKE:-0}" != 1 ]]; then
  CRL_NAME="$RUN"
  if sdk_as_person revoke --ca-cert "$CA_CERT" --ca-key "$CA_KEY" --crl "$WORK/revoked.crl" \
    --trust-anchor-arn "$TRUST_ANCHOR" --certificate "$CONF/enrolment/certificate.pem" \
    --name "$CRL_NAME" 2>"$WORK/revoke.err" && grep -q "^imported" "$WORK/revoke.err"; then
    record PASS "revoke: CRL $CRL_NAME imported with the test certificate's serial"
    echo "Waiting up to ${CRL_WAIT}s for the refusal"
    waited=0
    revoked=0
    while ((waited <= CRL_WAIT)); do
      if ! "$AGENT" --root "$ROOT" check >"$WORK/after.out" 2>&1; then
        grep -q "Roles Anywhere refused" "$WORK/after.out" && { revoked=1; break; }
      fi
      sleep 15
      waited=$((waited + 15))
    done
    if ((revoked)); then
      record PASS "the revoked installation is refused" "after ~${waited}s"
    else
      record FAIL "the revoked installation is refused" "$(tail -1 "$WORK/after.out")"
    fi
  else
    record FAIL "revoke" "$(tail -1 "$WORK/revoke.err")"
  fi
fi

# --- Summary --------------------------------------------------------------

cat <<EOF

----- paste below into decision 0001, Verification -----
date: $(date -u +%Y-%m-%d)
sdk: $("$SDK" --version)
agent: $("$AGENT" --version)
aws_signing_helper: $("$HELPER" version 2>/dev/null || echo unknown)
$(aws --version 2>&1)
channel: $CHANNEL (no index before the run)
$(printf '%s\n' "${RESULTS[@]}")
result: $( ((FAILED)) && echo FAIL || echo PASS)
---------------------------------------------------------
EOF
exit "$FAILED"
