#!/usr/bin/env bash
# Checks how Docker Compose hands a module its secret files (decision 0009,
# point 2, Verification V1), before the agent's secrets adapter builds on
# it. Local only: one throwaway project, invented values, no account.
#
# A module laid out as the agent lays it out: `<module>/secrets/` owned by
# root with mode 0700, each file owned by uid 1000 with mode 0400 (made by a
# root helper container), named in `compose.yml` as a Compose file secret,
# `${QD_SECRETS_DIR:?}/<name>`:
#
#   1. A service running as uid 1000 reads `/run/secrets/<name>`; one
#      running as uid 1001 cannot.
#   2. This host user (not root) can neither list the folder nor read a
#      file in it.
#   3. The long syntax with `uid` and `mode`: whether Compose without Swarm
#      honours them (expected: ignored, the file keeps its own owner and
#      mode).
#   4. A new file written aside and renamed over the old one: whether the
#      running container sees it (expected: no), after `up -d` (expected:
#      no, nothing changed for Compose) and after `up -d --force-recreate`
#      (expected: yes).
#   5. `docker compose config` and `docker inspect` show the file's path and
#      never its content.
#
# The project's containers and network, and the root-owned files, are
# removed on exit, also on failure. Run alone, under the machine lock: no
# build at the same time (AGENTS.md).
#
#   tools/verify/module-secrets.sh
#
# Environment:
#   QD_IMAGE   a local image to run, by tag (default alpine:3); it must have
#              a registry digest (pulled, not built here)
#
# Needs: docker with the compose plugin; not run as root.

set -euo pipefail

TAG="${QD_IMAGE:-alpine:3}"
PROJECT="qs-verify-secrets-$(date -u +%Y%m%dt%H%M%Sz)-$RANDOM"
WORK="$(mktemp -d)"
MODULE="$WORK/opt/quality-data/qs-verify"
SECRETS="$MODULE/secrets"
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

# As root, in a throwaway container with the work folder mounted.
as_root() {
  docker run --rm --network none --user 0:0 -v "$WORK:$WORK" --entrypoint sh "$IMAGE" -c "$1"
}

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local rc=$?
  set +e
  if [[ -e "$MODULE/current" ]]; then
    compose down -v --remove-orphans --timeout 1 >/dev/null 2>&1
  fi
  docker ps -aq --filter "label=com.docker.compose.project=$PROJECT" | xargs -r docker rm -f -v >/dev/null
  docker network ls -q --filter "label=com.docker.compose.project=$PROJECT" | xargs -r docker network rm >/dev/null
  if [[ -n "${IMAGE:-}" && -d "$SECRETS" ]]; then
    as_root "rm -rf '$SECRETS'" >/dev/null 2>&1
  fi
  if [[ -n "$(docker ps -aq --filter "label=com.docker.compose.project=$PROJECT")" ]]; then
    echo "WARN: remove what is left of the project $PROJECT by hand"
    rc=1
  fi
  rm -rf "$WORK" || {
    echo "WARN: remove $WORK by hand"
    rc=1
  }
  exit "$rc"
}
trap cleanup EXIT

[[ "$(id -u)" != 0 ]] || die "run it as a user who is not root"
docker compose version >/dev/null || die "docker compose is not installed"
IMAGE="$(docker image inspect "$TAG" --format '{{index .RepoDigests 0}}' 2>/dev/null)" ||
  die "$TAG is not a local image with a registry digest (docker pull $TAG)"

# Invented values, written by root: one file per case.
ONE="qs-verify-one-$RANDOM$RANDOM"
TWO="qs-verify-two-$RANDOM$RANDOM"
NEW="qs-verify-new-$RANDOM$RANDOM"
mkdir -p "$MODULE/1.0.0"
as_root "mkdir -p '$SECRETS' && chmod 0700 '$SECRETS' &&
  for f in secret-key:$ONE other-key:$TWO; do
    n=\${f%%:*}; printf %s \${f#*:} > '$SECRETS'/.\$n.tmp &&
    chmod 0400 '$SECRETS'/.\$n.tmp && sync && mv '$SECRETS'/.\$n.tmp '$SECRETS'/\$n &&
    chown 1000:1000 '$SECRETS'/\$n
  done"

cat >"$MODULE/1.0.0/compose.yml" <<'EOF'
services:
  reader:
    image: ${IMAGE_APP:?IMAGE_APP is the agent's}
    command: ["sleep", "3600"]
    stop_grace_period: 1s
    user: "1000:1000"
    secrets: [secret-key]
  other:
    image: ${IMAGE_APP}
    command: ["sleep", "3600"]
    stop_grace_period: 1s
    user: "1001:1001"
    secrets: [secret-key]
  long:
    image: ${IMAGE_APP}
    command: ["sleep", "3600"]
    stop_grace_period: 1s
    user: "0:0"
    secrets:
      - source: other-key
        target: other-key
        uid: "1001"
        gid: "1001"
        mode: 0444
secrets:
  secret-key:
    file: ${QD_SECRETS_DIR:?}/secret-key
  other-key:
    file: ${QD_SECRETS_DIR:?}/other-key
EOF
printf 'IMAGE_APP=%s\nQD_SECRETS_DIR=%s\n' "$IMAGE" "$SECRETS" >"$MODULE/.env"
ln -s 1.0.0 "$MODULE/current"
compose up -d --no-build >"$WORK/up.log" 2>&1 || die "up: $(tail -1 "$WORK/up.log")"

# --- 1. who reads the file in a container -------------------------------------

got="$(compose exec -T reader cat /run/secrets/secret-key 2>&1)" || true
if [[ "$got" == "$ONE" ]]; then
  record PASS "a service running as uid 1000 reads /run/secrets/secret-key"
else
  record FAIL "a service running as uid 1000 reads /run/secrets/secret-key" "${got:0:80}"
fi
if compose exec -T other cat /run/secrets/secret-key >/dev/null 2>"$WORK/other.log"; then
  record FAIL "a service running as uid 1001 cannot read it" "it could"
else
  record PASS "a service running as uid 1001 cannot read it" "$(tail -1 "$WORK/other.log")"
fi

# --- 2. this host user is kept out by the folder ------------------------------

if ls "$SECRETS" >/dev/null 2>&1 || cat "$SECRETS/secret-key" >/dev/null 2>&1; then
  record FAIL "a host user who is not root cannot list the folder or read a file" "uid $(id -u) could"
else
  record PASS "a host user who is not root cannot list the folder or read a file" \
    "uid $(id -u), the file's owner, is refused by the folder's 0700"
fi

# --- 3. the long syntax's uid and mode without Swarm --------------------------

stat_long="$(compose exec -T long stat -c '%u:%g %a' /run/secrets/other-key 2>&1)" || true
warned="$(grep -ci 'not supported\|ignored\|uid\|mode' "$WORK/up.log" || true)"
if [[ "$stat_long" == "1000:1000 400" ]]; then
  record PASS "the long syntax's uid and mode are ignored without Swarm" \
    "the file stays $stat_long; warnings in up's output: $warned"
else
  record FAIL "the long syntax's uid and mode are ignored without Swarm" "the file is $stat_long"
fi

# --- 4. a file renamed over the old one ---------------------------------------

as_root "printf %s $NEW > '$SECRETS'/.secret-key.next && chmod 0400 '$SECRETS'/.secret-key.next &&
  chown 1000:1000 '$SECRETS'/.secret-key.next && sync &&
  mv '$SECRETS'/.secret-key.next '$SECRETS'/secret-key"
seen() { compose exec -T reader cat /run/secrets/secret-key 2>/dev/null || true; }
check_seen() { # check_seen when expected(new|old)
  local got want
  got="$(seen)"
  want=$([[ "$2" == new ]] && echo "$NEW" || echo "$ONE")
  if [[ "$got" == "$want" ]]; then
    record PASS "a renamed-over file $1: the container sees the $2 value"
  else
    record FAIL "a renamed-over file $1: the container sees the $2 value" \
      "it sees $([[ "$got" == "$NEW" ]] && echo new || echo old)"
  fi
}
check_seen "in the running container" old
compose up -d --no-build >>"$WORK/up.log" 2>&1 || die "up -d: $(tail -1 "$WORK/up.log")"
check_seen "after up -d" old
compose up -d --no-build --force-recreate reader >>"$WORK/up.log" 2>&1 ||
  die "up --force-recreate: $(tail -1 "$WORK/up.log")"
check_seen "after up -d --force-recreate" new

# --- 5. Compose and Docker show the path, never the content -------------------

config="$(compose config 2>&1)"
inspect="$(docker inspect "$(compose ps -q reader)" "$(compose ps -q long)")"
leaks=0
for v in "$ONE" "$TWO" "$NEW"; do
  if grep -qF "$v" <<<"$config$inspect"; then leaks=$((leaks + 1)); fi
done
if [[ "$leaks" == 0 ]] && grep -qF "$SECRETS/secret-key" <<<"$config" &&
  grep -qF "$SECRETS/secret-key" <<<"$inspect"; then
  record PASS "compose config and docker inspect show the path and never the content"
else
  record FAIL "compose config and docker inspect show the path and never the content" \
    "values found: $leaks"
fi

echo
echo "Docker $(docker version --format '{{.Server.Version}}'), $(docker compose version --short), $IMAGE"
printf '%s\n' "${RESULTS[@]}" | grep -c '^PASS' | xargs printf 'passed: %s\n'
exit "$FAILED"
