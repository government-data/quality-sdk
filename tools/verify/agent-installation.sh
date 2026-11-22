#!/usr/bin/env bash
# Checks the agent's own calls on a real installation (decision 0004 step
# 5; the "first real test installation" checks decision 0003 left): run by
# a person, as root, on the test machine after `packaging/install.sh` and
# `quality-data-agent install` finished. Never by an agent or in CI: it
# calls the real account with the installation's enrolment certificate.
#
#   1. The machine gives no AWS credentials: IMDSv1 is refused and IMDSv2
#      has no instance profile (`iam/security-credentials/` is 404).
#   Inside the agent's systemd sandbox (the [Service] lines of
#   packaging/systemd/quality-data-agent.service, applied with
#   `systemd-run`, as tools/verify/agent-unit.sh does):
#   2. `aws_signing_helper credential-process` with the enrolment
#      certificate gives temporary credentials (Roles Anywhere).
#   3. `quality-data-agent check`: the signed index read from S3 and
#      verified, the registry's credential, one report of kind `check`.
#   4. ECR: `GetAuthorizationToken` with those credentials, `docker login`
#      in a private configuration and the pull of the helper image.
#   5. `updater/v1`: `GET /updater/v1/version` with the agent's internal
#      certificate (mTLS, `[api] connect` honoured).
#   6. `quality-data-agent run --once`: `PUT /updater/v1/modules` (no
#      warning that quality-data was not told), and the renewal of the
#      internal certificate when it is due (less than 30 days left;
#      otherwise SKIP: forcing it is decision 0004's Open item).
#
# The service is stopped while it runs (one agent holds the lock) and
# started again on exit, also on failure. The report of kind `check` stays
# in the bucket (the installation may not delete it).
#
#   sudo tools/verify/agent-installation.sh
#
# Needs: systemd as PID 1, docker, curl (with --aws-sigv4), jq, openssl,
# and the installation laid out under /etc/quality-data, /opt/quality-data
# and /var/lib/quality-data.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
UNIT_FILE="${QD_UNIT_FILE:-/etc/systemd/system/quality-data-agent.service}"
[[ -f "$UNIT_FILE" ]] || UNIT_FILE="$ROOT/packaging/systemd/quality-data-agent.service"
AGENT="${QD_AGENT:-/usr/local/bin/quality-data-agent}"
CONFIG=/etc/quality-data/agent.toml
INTERNAL=/etc/quality-data/internal
RUN="qs-verify-installation-$(date -u +%Y%m%dt%H%M%Sz)-$RANDOM"
WORK="$(mktemp -d)"
WAS_ACTIVE=0
FAILED=0
RESULTS=()

record() { # record PASS|FAIL|SKIP name [detail]
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
  local rc=$?
  set +e
  systemctl stop "$RUN.service" >/dev/null 2>&1
  systemctl reset-failed "$RUN.service" >/dev/null 2>&1
  if ((WAS_ACTIVE)); then
    systemctl start quality-data-agent.service || {
      echo "WARN: start quality-data-agent again by hand"
      rc=1
    }
  fi
  rm -rf "$WORK"
  exit "$rc"
}
trap cleanup EXIT

# toml <section, "" for the top> <key>: a `key = "value"` line's value.
toml() {
  awk -v sec="$1" -v key="$2" '
    /^[[:space:]]*\[/ { cur = $0; gsub(/[][[:space:]]/, "", cur); next }
    cur == sec && $1 == key && $2 == "=" {
      v = $0; sub(/^[^=]*=[[:space:]]*"/, "", v); sub(/".*$/, "", v); print v; exit
    }' "$CONFIG"
}

[[ $EUID -eq 0 ]] || die "run it as root (sudo), as the agent runs"
[[ "$(ps -p 1 -o comm=)" == systemd ]] || die "systemd is not PID 1 here"
for tool in curl jq openssl docker; do
  command -v "$tool" >/dev/null || die "$tool is not installed"
done
curl --help all | grep -q -- --aws-sigv4 || die "this curl has no --aws-sigv4"
[[ -x "$AGENT" ]] || die "$AGENT is not installed"
[[ -f "$CONFIG" ]] || die "$CONFIG is missing: the installation is not laid out"

REGION="$(toml aws region)"
ROLE="$(toml aws role_arn)"
HELPER="$(toml aws signing_helper)"
HELPER="${HELPER:-/usr/local/bin/aws_signing_helper}"
API_URL="$(toml api url)"
CONNECT="$(toml api connect)"
HELPER_IMAGE="$(toml "" helper_image)"
if [[ -z "$HELPER_IMAGE" ]]; then
  HELPER_IMAGE="$(cut -d: -f5 <<<"$ROLE").dkr.ecr.$REGION.amazonaws.com/quality-data/tools/busybox@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec"
fi
[[ -n "$API_URL" ]] || die "$CONFIG has no [api] url"
# `[api] connect` as curl's --connect-to: the URL's host and port to that address.
HOSTPORT="${API_URL#https://}"
HOSTPORT="${HOSTPORT%%/*}"
[[ "$HOSTPORT" == *:* ]] || HOSTPORT="$HOSTPORT:443"
CONNECT_TO=""
[[ -z "$CONNECT" ]] || CONNECT_TO="$HOSTPORT:$CONNECT"

# --- 1. no credentials from the instance ----------------------------------------

IMDS=http://169.254.169.254/latest
v1="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$IMDS/meta-data/" || true)"
if [[ "$v1" == 401 ]]; then
  record PASS "IMDSv1 is refused" "$v1"
else
  record FAIL "IMDSv1 is refused" "answered $v1"
fi
token="$(curl -sf --max-time 5 -X PUT "$IMDS/api/token" -H 'X-aws-ec2-metadata-token-ttl-seconds: 60' || true)"
if [[ -z "$token" ]]; then
  record FAIL "IMDSv2 answers a token" "no token"
else
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 -H "X-aws-ec2-metadata-token: $token" \
    "$IMDS/meta-data/iam/security-credentials/" || true)"
  if [[ "$code" == 404 ]]; then
    record PASS "no instance profile: IMDSv2 gives no credentials" "$code"
  else
    record FAIL "no instance profile: IMDSv2 gives no credentials" "answered $code"
  fi
fi

# --- 2 to 6. inside the sandbox -------------------------------------------------

# The sandbox: every [Service] line but what starts and restarts it.
PROPS=()
while IFS= read -r line; do
  PROPS+=(-p "$line")
done < <(awk '/^\[/{s=($0=="[Service]")} s && /^[A-Z][A-Za-z]*=/' "$UNIT_FILE" |
  grep -Ev '^(Type|ExecStart|Restart|RestartSec|TimeoutStopSec)=')

if systemctl is-active --quiet quality-data-agent.service; then
  WAS_ACTIVE=1
  systemctl stop quality-data-agent.service
fi
NOT_AFTER="$(openssl x509 -in "$INTERNAL/certificate.pem" -noout -enddate 2>/dev/null | cut -d= -f2 || true)"
DUE=0
openssl x509 -in "$INTERNAL/certificate.pem" -noout -checkend $((30 * 86400)) >/dev/null 2>&1 || DUE=1

cat >"$WORK/inside.sh" <<'EOF'
set -u
say() { if eval "$2" >/tmp/out 2>&1; then echo "PASS  $1"; else echo "FAIL  $1 ($(tail -1 /tmp/out | cut -c1-200))"; fi; }
E=/etc/quality-data/enrolment
say "aws_signing_helper gives Roles Anywhere credentials" \
  '"$HELPER" credential-process --certificate "$E/certificate.pem" --private-key "$E/private-key.pem" \
     --trust-anchor-arn "$TRUST_ANCHOR" --profile-arn "$PROFILE" --role-arn "$ROLE" --region "$REGION" \
     >/tmp/creds.json && jq -e ".AccessKeyId and .SessionToken" /tmp/creds.json'
say "quality-data-agent check: the S3 index verified, the registry, a report" '"$AGENT" check'
say "ECR GetAuthorizationToken, docker login and the helper image pulled" '(
  c=$(mktemp -d)
  curl -sf --aws-sigv4 "aws:amz:$REGION:ecr" \
    --user "$(jq -r .AccessKeyId /tmp/creds.json):$(jq -r .SecretAccessKey /tmp/creds.json)" \
    -H "x-amz-security-token: $(jq -r .SessionToken /tmp/creds.json)" \
    -H "X-Amz-Target: AmazonEC2ContainerRegistry_V20150921.GetAuthorizationToken" \
    -H "Content-Type: application/x-amz-json-1.1" -d "{}" "https://api.ecr.$REGION.amazonaws.com/" >/tmp/ecr.json &&
  jq -r ".authorizationData[0].authorizationToken" /tmp/ecr.json | base64 -d | cut -d: -f2- |
    docker --config "$c" login --username AWS --password-stdin "${HELPER_IMAGE%%/*}" &&
  docker --config "$c" pull "$HELPER_IMAGE"; rc=$?; rm -rf "$c"; exit $rc)'
say "updater/v1 GET /version over mTLS" '
  I=/etc/quality-data/internal
  curl -sf --max-time 30 ${CONNECT_TO:+--connect-to "$CONNECT_TO"} --cacert "$I/ca.pem" \
    --cert "$I/certificate.pem" --key "$I/private-key.pem" "$API_URL/updater/v1/version"'
"$AGENT" run --once >/tmp/run.log 2>&1
echo "RUN $?"
say "run --once tells quality-data the modules (PUT /updater/v1/modules)" \
  '! grep -q "was not told the installed modules" /tmp/run.log'
grep -o "the internal certificate .*" /tmp/run.log | sed "s/^/LOG  /"
EOF

systemd-run --wait --pipe --quiet --collect --unit "$RUN" "${PROPS[@]}" \
  --setenv=PATH="$PATH" --setenv=AGENT="$AGENT" --setenv=HELPER="$HELPER" \
  --setenv=REGION="$REGION" --setenv=ROLE="$ROLE" \
  --setenv=TRUST_ANCHOR="$(toml aws trust_anchor_arn)" --setenv=PROFILE="$(toml aws profile_arn)" \
  --setenv=HELPER_IMAGE="$HELPER_IMAGE" --setenv=API_URL="$API_URL" --setenv=CONNECT_TO="$CONNECT_TO" \
  "$(command -v bash)" -s <"$WORK/inside.sh" >"$WORK/inside.log" 2>&1 || true
while IFS= read -r line; do
  case "$line" in
  PASS\ \ *) record PASS "${line#PASS  }" ;;
  FAIL\ \ *) record FAIL "${line#FAIL  }" ;;
  "RUN 0") record PASS "quality-data-agent run --once inside the sandbox" ;;
  RUN\ *) record FAIL "quality-data-agent run --once inside the sandbox" "exit ${line#RUN }" ;;
  LOG\ \ *) echo "${line#LOG  }" ;;
  esac
done <"$WORK/inside.log"
grep -q '^PASS\|^FAIL' "$WORK/inside.log" ||
  record FAIL "the sandboxed service ran" "$(tail -3 "$WORK/inside.log" | tr '\n' ' ')"

AFTER="$(openssl x509 -in "$INTERNAL/certificate.pem" -noout -enddate 2>/dev/null | cut -d= -f2 || true)"
if ((!DUE)); then
  record SKIP "the internal certificate's renewal" "not due: it ends $NOT_AFTER, more than 30 days away"
elif [[ -n "$AFTER" && "$AFTER" != "$NOT_AFTER" ]]; then
  record PASS "the internal certificate was renewed inside the sandbox" "$NOT_AFTER -> $AFTER"
else
  record FAIL "the internal certificate was renewed inside the sandbox" "still ends ${AFTER:-unreadable}"
fi

# --- Summary --------------------------------------------------------------

cat <<EOF

----- paste below into decision 0003, Verification -----
date: $(date -u +%Y-%m-%d)
$(sed -n "s/^PRETTY_NAME=//p" /etc/os-release | tr -d \"); kernel $(uname -r); $(systemctl --version | head -1)
$(docker version --format 'docker {{.Server.Version}}'); $("$AGENT" --version); $("$HELPER" version 2>/dev/null | head -1)
$(printf '%s\n' "${RESULTS[@]}")
result: $( ((FAILED)) && echo FAIL || echo PASS)
---------------------------------------------------------
EOF
exit "$FAILED"
