#!/usr/bin/env bash
# Checks how Docker Compose behaves where the agent's Docker Compose adapter
# builds on it (decision 0003, step 7), before it is built. Local only: one
# throwaway project, no account.
#
# A module laid out as the agent lays it out:
#
#   <work>/opt/quality-data/qs-verify/1.0.0/compose.yml
#   <work>/opt/quality-data/qs-verify/2.0.0/compose.yml
#   <work>/opt/quality-data/qs-verify/current -> 1.0.0, then 2.0.0
#   <work>/opt/quality-data/qs-verify/.env     IMAGE_APP=<image>@sha256:<digest>
#
# run as `docker compose --project-name P --project-directory <module>/current
# --file <module>/current/compose.yml --env-file <module>/.env`:
#
#   1. `${IMAGE_APP}` comes from the `.env` beside the versions, and the
#      container runs that image by digest.
#   2. `up -d --wait` returns once every healthcheck is healthy, and fails
#      (within `--wait-timeout`) when one is not.
#   3. `ps --all --format json` gives each container's service, state and
#      health, one JSON object per line.
#   4. Switching `current` to another version keeps the project and its
#      named volume (the data is there), and `--remove-orphans` removes a
#      service the new version no longer has.
#   5. `down` without `-v` keeps the named volume; `up` again finds the data.
#   6. When `current` moves to a version whose image and compose are the
#      same, plain `up` keeps the container and the old version's
#      bind-mounted file; `up --force-recreate`, which the adapter runs
#      (decision 0005), gives the new version's.
#
# Every other `up` is the adapter's, with `--force-recreate`.
#
# The project's containers, network and volume are removed on exit, also on
# failure. Run alone: no build at the same time (AGENTS.md).
#
#   tools/verify/compose.sh
#
# Environment:
#   QD_IMAGE   a local image to run, by tag (default alpine:3); it must have a
#              registry digest (pulled, not built here)
#
# Needs: docker with the compose plugin, jq.

set -euo pipefail

TAG="${QD_IMAGE:-alpine:3}"
PROJECT="qs-verify-compose-$(date -u +%Y%m%dt%H%M%Sz)-$RANDOM"
WORK="$(mktemp -d)"
MODULE="$WORK/opt/quality-data/qs-verify"
FAILED=0
RESULTS=()

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

compose() {
  docker compose --project-name "$PROJECT" --project-directory "$MODULE/current" \
    --file "$MODULE/current/compose.yml" --env-file "$MODULE/.env" "$@"
}

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local rc=$?
  set +e
  if [[ -e "$MODULE/current" ]]; then
    compose down -v --remove-orphans --timeout 1 >/dev/null 2>&1
  fi
  docker ps -aq --filter "label=com.docker.compose.project=$PROJECT" | xargs -r docker rm -f -v >/dev/null
  docker volume ls -q --filter "label=com.docker.compose.project=$PROJECT" | xargs -r docker volume rm >/dev/null
  docker network ls -q --filter "label=com.docker.compose.project=$PROJECT" | xargs -r docker network rm >/dev/null
  if [[ -n "$(docker ps -aq --filter "label=com.docker.compose.project=$PROJECT")$(docker volume ls -q --filter "label=com.docker.compose.project=$PROJECT")" ]]; then
    echo "WARN: remove what is left of the project $PROJECT by hand"
    rc=1
  fi
  rm -rf "$WORK"
  exit "$rc"
}
trap cleanup EXIT

command -v jq >/dev/null || die "jq is not installed"
docker compose version >/dev/null || die "docker compose is not installed"
IMAGE="$(docker image inspect "$TAG" --format '{{index .RepoDigests 0}}' 2>/dev/null)" ||
  die "$TAG is not a local image with a registry digest (docker pull $TAG)"

# version <dir> <healthcheck: true|false> <with the old service: 1|0>
version() {
  mkdir -p "$MODULE/$1"
  {
    cat <<EOF
services:
  app:
    image: \${IMAGE_APP:?IMAGE_APP is the agent's}
    command: ["sleep", "3600"]
    stop_grace_period: 1s
    volumes: ["data:/data"]
    healthcheck:
      test: ["CMD", "$2"]
      interval: 1s
      timeout: 2s
      retries: 2
EOF
    if [[ "$3" == 1 ]]; then
      cat <<EOF
  old:
    image: \${IMAGE_APP}
    command: ["sleep", "3600"]
    stop_grace_period: 1s
EOF
    fi
    cat <<EOF
volumes:
  data: {}
EOF
  } >"$MODULE/$1/compose.yml"
}
version 1.0.0 true 1
version 2.0.0 true 0
version 3.0.0 false 0
printf 'KEPT=yes\nIMAGE_APP=%s\n' "$IMAGE" >"$MODULE/.env"
ln -s 1.0.0 "$MODULE/current"

# --- 1 and 2. Up, by digest, waiting for health -----------------------------

if compose up -d --no-build --force-recreate --remove-orphans --wait --wait-timeout 60 >"$WORK/up.log" 2>&1; then
  record PASS "up -d --wait returns once the healthcheck is healthy"
else
  record FAIL "up -d --wait" "$(tail -1 "$WORK/up.log")"
  exit 1
fi
running="$(docker inspect "$(compose ps -q app)" --format '{{.Config.Image}}')"
if [[ "$running" == "$IMAGE" ]]; then
  record PASS "\${IMAGE_APP} comes from the .env beside the versions" "$IMAGE"
else
  record FAIL "\${IMAGE_APP} comes from the .env" "runs $running"
fi

# --- 3. What ps says -----------------------------------------------------------

compose ps --all --format json >"$WORK/ps.json"
lines="$(wc -l <"$WORK/ps.json")"
if jq -e -s 'length == 2 and all(.[]; has("Service") and has("State") and has("Health"))
    and (map(select(.Service == "app"))[0] | .State == "running" and .Health == "healthy")
    and (map(select(.Service == "old"))[0] | .State == "running" and .Health == "")' \
  "$WORK/ps.json" >/dev/null; then
  record PASS "ps --all --format json: one object per line with Service, State and Health" \
    "$lines lines; no healthcheck is Health \"\""
else
  record FAIL "ps --all --format json" "$(head -c 300 "$WORK/ps.json")"
fi
compose exec -T app sh -c 'echo kept > /data/proof'

# --- 4. Another version --------------------------------------------------------

ln -sfn 2.0.0 "$MODULE/current"
compose up -d --no-build --force-recreate --remove-orphans --wait --wait-timeout 60 >"$WORK/up2.log" 2>&1 ||
  record FAIL "up of 2.0.0" "$(tail -1 "$WORK/up2.log")"
if [[ "$(compose exec -T app cat /data/proof 2>/dev/null)" == kept ]]; then
  record PASS "switching current to 2.0.0 keeps the project and its named volume"
else
  record FAIL "switching current keeps the named volume"
fi
if [[ -z "$(docker ps -aq --filter "label=com.docker.compose.project=$PROJECT" \
  --filter "label=com.docker.compose.service=old")" ]]; then
  record PASS "--remove-orphans removes a service the new version does not have"
else
  record FAIL "--remove-orphans removes a service the new version does not have"
fi

# --- 2. A healthcheck that fails -------------------------------------------------

ln -sfn 3.0.0 "$MODULE/current"
start=$SECONDS
if compose up -d --no-build --force-recreate --remove-orphans --wait --wait-timeout 20 >"$WORK/up3.log" 2>&1; then
  record FAIL "up -d --wait fails on a healthcheck that fails" "it returned 0"
else
  record PASS "up -d --wait fails on a healthcheck that fails" \
    "after $((SECONDS - start))s: $(grep -iom1 'unhealthy\|timeout[^"]*' "$WORK/up3.log" || tail -1 "$WORK/up3.log")"
fi
if compose ps --all --format json | jq -e -s 'any(.[]; .Health == "unhealthy")' >/dev/null; then
  record PASS "ps then says the container is unhealthy"
else
  record FAIL "ps then says the container is unhealthy"
fi

# --- 5. Down keeps the data ------------------------------------------------------

ln -sfn 2.0.0 "$MODULE/current"
compose down --timeout 1 >/dev/null 2>&1
if [[ -z "$(compose ps -aq)" ]] &&
  compose up -d --no-build --force-recreate --wait --wait-timeout 60 >/dev/null 2>&1 &&
  [[ "$(compose exec -T app cat /data/proof 2>/dev/null)" == kept ]]; then
  record PASS "down without -v keeps the named volume; up again finds the data"
else
  record FAIL "down without -v keeps the named volume"
fi

# --- 6. A bind-mounted file of the new version ---------------------------------

compose down --timeout 1 >/dev/null 2>&1
for v in 4.0.0 5.0.0; do
  mkdir -p "$MODULE/$v"
  echo "$v" >"$MODULE/$v/conf.txt"
  cat >"$MODULE/$v/compose.yml" <<EOF
services:
  app:
    image: \${IMAGE_APP}
    command: ["sleep", "3600"]
    stop_grace_period: 1s
    volumes: ["./conf.txt:/conf.txt:ro"]
EOF
done
conf() { compose exec -T app cat /conf.txt 2>/dev/null; }
ln -sfn 4.0.0 "$MODULE/current"
compose up -d --no-build --force-recreate --wait --wait-timeout 60 >/dev/null 2>&1
ln -sfn 5.0.0 "$MODULE/current"
compose up -d --no-build --wait --wait-timeout 60 >/dev/null 2>&1
stale="$(conf)"
if [[ "$stale" == 4.0.0 ]]; then
  record PASS "plain up keeps the container and the old version's bind-mounted file" "reads $stale"
else
  record FAIL "plain up keeps the old version's bind-mounted file" "reads ${stale:-nothing}"
fi
compose up -d --no-build --force-recreate --wait --wait-timeout 60 >/dev/null 2>&1
fresh="$(conf)"
if [[ "$fresh" == 5.0.0 ]]; then
  record PASS "up --force-recreate gives the new version's bind-mounted file" "reads $fresh"
else
  record FAIL "up --force-recreate gives the new version's bind-mounted file" "reads ${fresh:-nothing}"
fi

# --- Summary --------------------------------------------------------------

cat <<EOF

----- paste below into decision 0003, Verification -----
date: $(date -u +%Y-%m-%d)
$(docker version --format 'docker {{.Server.Version}}'); $(docker compose version)
image: $IMAGE
$(printf '%s\n' "${RESULTS[@]}")
result: $( ((FAILED)) && echo FAIL || echo PASS)
---------------------------------------------------------
EOF
exit "$FAILED"
