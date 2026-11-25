#!/usr/bin/env bash
# Checks the way a cluster pulls with the registry token the core hands out
# (decision 0013): a user and a password, as ECR's token decodes to
# (`AWS:<password>`), logged in to in a configuration directory of its own
# and pulled by digest, as `agent::aws::images::Docker` does. Local only: a
# `registry:2` with htpasswd stands in for ECR, no account is called.
#
#   1. `docker --config <dir> login --password-stdin` with the user and the
#      password succeeds, and the credential is written only under <dir>
#      (not in ~/.docker).
#   2. A pull by digest with that configuration succeeds.
#   3. Without the credential the registry refuses the image (401).
#   4. A wrong password is refused at login.
#
# The container, network, image tags and directories are removed on exit,
# also on failure. Run alone: no build at the same time (AGENTS.md).
#
#   tools/verify/registry-login.sh
#
# Environment:
#   QD_REGISTRY_IMAGE  the registry (default registry:2)
#   QD_IMAGE           a local image to push and pull (default alpine:3)
#   QD_PY_IMAGE        makes the bcrypt hash (default python:3.12-alpine)
#
# Needs: docker.

set -euo pipefail

REGISTRY_IMAGE="${QD_REGISTRY_IMAGE:-registry:2}"
IMAGE="${QD_IMAGE:-alpine:3}"
PY_IMAGE="${QD_PY_IMAGE:-python:3.12-alpine}"
NAME="qs-verify-registry-$RANDOM"
WORK="$(mktemp -d)"
FAILED=0

record() { # record PASS|FAIL name [detail]
  echo "$1  $2${3:+ ($3)}"
  if [[ "$1" == FAIL ]]; then FAILED=1; fi
}

cleanup() {
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  docker image rm "${PUSHED:-none}" "${BY_DIGEST:-none}" >/dev/null 2>&1 || true
  docker images --digests --format '{{.Repository}}@{{.Digest}}' \
    | grep "^${HOST:-none}/qs-verify/" | xargs -r docker image rm >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

USER_NAME=AWS
PASSWORD="$(head -c 48 /dev/urandom | base64 | tr -d '/+=\n')"
mkdir -p "$WORK/auth" "$WORK/push" "$WORK/pull" "$WORK/none"

# The htpasswd line (bcrypt), made in a container: the password goes in on
# standard input, never on a command line.
printf '%s' "$PASSWORD" | docker run --rm -i "$PY_IMAGE" python -I -c '
import crypt, sys
pw = sys.stdin.read()
print("AWS:" + crypt.crypt(pw, crypt.mksalt(crypt.METHOD_BLOWFISH)))
' 2>/dev/null >"$WORK/auth/htpasswd"

docker run -d --name "$NAME" -p 127.0.0.1::5000 \
  -v "$WORK/auth:/auth:ro" \
  -e REGISTRY_AUTH=htpasswd -e REGISTRY_AUTH_HTPASSWD_REALM=qs-verify \
  -e REGISTRY_AUTH_HTPASSWD_PATH=/auth/htpasswd \
  "$REGISTRY_IMAGE" >/dev/null
PORT="$(docker port "$NAME" 5000/tcp | head -1 | sed 's/.*://')"
HOST="127.0.0.1:$PORT"
for _ in $(seq 1 30); do
  curl -s -o /dev/null "http://$HOST/v2/" && break
  sleep 1
done

login() { # login <config dir> <password>
  printf '%s' "$2" | docker --config "$1" login --username "$USER_NAME" \
    --password-stdin "$HOST" >/dev/null 2>&1
}

# Push the image into the registry, logged in with a configuration of its own.
login "$WORK/push" "$PASSWORD"
PUSHED="$HOST/qs-verify/image:one"
docker tag "$IMAGE" "$PUSHED"
DIGEST="$(docker --config "$WORK/push" push "$PUSHED" | sed -n 's/.*digest: \(sha256:[0-9a-f]*\).*/\1/p')"
docker image rm "$PUSHED" >/dev/null
BY_DIGEST="$HOST/qs-verify/image@$DIGEST"

if login "$WORK/pull" "$PASSWORD"; then
  record PASS "login with the user and the password"
else
  record FAIL "login with the user and the password"
fi
STORED="$(printf '%s:%s' "$USER_NAME" "$PASSWORD" | base64 -w0)"
if grep -qs "$STORED" "$WORK/pull/config.json" \
  && ! grep -rqs -e "$STORED" -e "$PASSWORD" "$HOME/.docker" 2>/dev/null; then
  record PASS "the credential is under the configuration directory only"
else
  record FAIL "the credential is under the configuration directory only"
fi
if docker --config "$WORK/pull" pull --quiet "$BY_DIGEST" >/dev/null 2>&1; then
  record PASS "pull by digest with that configuration" "$DIGEST"
else
  record FAIL "pull by digest with that configuration"
fi
# Asked of the registry itself: Docker may serve a digest it already holds
# without asking the registry.
ANON="$(curl -s -o /dev/null -w '%{http_code}' \
  -H 'Accept: application/vnd.oci.image.index.v1+json' \
  "http://$HOST/v2/qs-verify/image/manifests/$DIGEST")"
if [[ "$ANON" == 401 ]]; then
  record PASS "the registry refuses the image without the credential" "$ANON"
else
  record FAIL "the registry refuses the image without the credential" "$ANON"
fi
if login "$WORK/none" "not-$PASSWORD"; then
  record FAIL "a wrong password is refused"
else
  record PASS "a wrong password is refused"
fi

echo "docker $(docker version --format '{{.Server.Version}}'), $REGISTRY_IMAGE"
if [[ $FAILED == 0 ]]; then echo "PASS"; else echo "FAIL"; exit 1; fi
