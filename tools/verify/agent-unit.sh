#!/usr/bin/env bash
# Checks that the agent's systemd sandbox (decision 0003 point 6) still lets
# it do its work: the sandbox lines of
# packaging/systemd/quality-data-agent.service are applied to a transient
# system service (`systemd-run`, as root, as the agent runs) that runs, from
# inside the sandbox, what the agent runs. Local only: no account.
#
#   1. The unit file passes `systemd-analyze verify` (its ExecStart pointed
#      at `true`); its exposure (`systemd-analyze security`) is printed.
#   2. Inside: no new privileges; Docker's socket answers (`docker version`)
#      and `docker compose version` finds the plugin; a `docker --config` in
#      a private /tmp works (the image puller's login).
#   3. Inside: a throwaway module under /opt/quality-data runs as the agent
#      runs it (`up --detach --no-build --wait`, `exec -T`, `ps --all
#      --format json`, `cp <service>:<path> -` into /var/lib/quality-data,
#      `stop`, the volume emptied with `docker run --volumes-from`, `cp -`
#      back, `down`), and `timeout` and `df -Pk` work.
#   4. Inside: files are written, renamed and removed in /var/lib/quality-data,
#      /opt/quality-data and /etc/quality-data/internal, and a file in
#      /etc/quality-data is read; writing anywhere else (/etc, /usr,
#      /var/lib, /etc/quality-data) fails; /home is empty; the host's /tmp
#      is not seen.
#
# It needs sudo without a password (systemd-run as root). It creates the
# three directories if they are missing and removes what it created on
# exit, with the transient unit, the module's containers, network and
# volumes, also on failure. Run alone: no build at the same time
# (AGENTS.md).
#
#   tools/verify/agent-unit.sh
#
# Environment:
#   QD_IMAGE   a local image to run, by tag (default alpine:3); it must have a
#              registry digest (pulled, not built here)
#
# Needs: sudo, systemd (systemd-run, systemd-analyze), docker with the
# compose plugin installed system-wide.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
UNIT_FILE="$ROOT/packaging/systemd/quality-data-agent.service"
TAG="${QD_IMAGE:-alpine:3}"
RUN="qs-verify-unit-$(date -u +%Y%m%dt%H%M%Sz)-$RANDOM"
MODULE="/opt/quality-data/$RUN"
WORK="$(mktemp -d)"
MARK="/tmp/$RUN.host"
CREATED=()
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

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local rc=$?
  set +e
  sudo -n systemctl stop "$RUN.service" >/dev/null 2>&1
  sudo -n systemctl reset-failed "$RUN.service" >/dev/null 2>&1
  docker rm -f "$RUN-empty" >/dev/null 2>&1
  docker ps -aq --filter "label=com.docker.compose.project=$RUN" | xargs -r docker rm -f -v >/dev/null
  docker volume ls -q --filter "label=com.docker.compose.project=$RUN" | xargs -r docker volume rm >/dev/null
  docker network ls -q --filter "label=com.docker.compose.project=$RUN" | xargs -r docker network rm >/dev/null
  sudo -n rm -rf "$MODULE" "/var/lib/quality-data/$RUN" "/etc/quality-data/internal/$RUN" \
    "/etc/quality-data/$RUN.read" "$MARK"
  local i
  for ((i = ${#CREATED[@]} - 1; i >= 0; i--)); do sudo -n rmdir "${CREATED[i]}"; done
  if [[ -n "$(docker ps -aq --filter "label=com.docker.compose.project=$RUN")" ]]; then
    echo "WARN: remove what is left of the project $RUN by hand"
    rc=1
  fi
  rm -rf "$WORK"
  exit "$rc"
}
trap cleanup EXIT

sudo -n true 2>/dev/null || die "sudo without a password is needed (systemd-run as root)"
[[ "$(ps -p 1 -o comm=)" == systemd ]] || die "systemd is not PID 1 here: the sandbox cannot be checked"
docker compose version >/dev/null || die "docker compose is not installed"
IMAGE="$(docker image inspect "$TAG" --format '{{index .RepoDigests 0}}' 2>/dev/null)" ||
  die "$TAG is not a local image with a registry digest (docker pull $TAG)"

# --- 1. the unit file -----------------------------------------------------------

sed "s#^ExecStart=.*#ExecStart=$(type -P true)#" "$UNIT_FILE" >"$WORK/quality-data-agent.service"
if systemd-analyze verify "$WORK/quality-data-agent.service" >"$WORK/verify.log" 2>&1; then
  record PASS "the unit passes systemd-analyze verify"
else
  record FAIL "the unit passes systemd-analyze verify" "$(tail -1 "$WORK/verify.log")"
fi
EXPOSURE="$(systemd-analyze security --offline=true "$WORK/quality-data-agent.service" 2>/dev/null |
  tail -1 | LC_ALL=C tr -cd '[:print:]' | sed 's/^ *//; s/ *$//')" || EXPOSURE="not available"

# The sandbox: every [Service] line but what starts and restarts it.
PROPS=()
while IFS= read -r line; do
  PROPS+=(-p "$line")
done < <(awk '/^\[/{s=($0=="[Service]")} s && /^[A-Z][A-Za-z]*=/' "$UNIT_FILE" |
  grep -Ev '^(Type|ExecStart|Restart|RestartSec|TimeoutStopSec)=')

# The directories the agent writes, created by the installer.
for d in /var/lib/quality-data /opt/quality-data /etc/quality-data /etc/quality-data/internal; do
  if [[ ! -d "$d" ]]; then
    sudo -n mkdir -m 0755 "$d"
    CREATED+=("$d")
  fi
done
sudo -n mkdir -p "$MODULE/1.0.0" "/var/lib/quality-data/$RUN"
sudo -n tee "$MODULE/1.0.0/compose.yml" >/dev/null <<'EOF'
services:
  app:
    image: ${IMAGE_APP:?IMAGE_APP is the agent's}
    command: ["sleep", "3600"]
    stop_grace_period: 1s
    healthcheck: { test: ["CMD", "true"], interval: 1s, retries: 3 }
    volumes: ["data:/data"]
volumes:
  data: {}
EOF
printf 'IMAGE_APP=%s\n' "$IMAGE" | sudo -n tee "$MODULE/.env" >/dev/null
sudo -n ln -s 1.0.0 "$MODULE/current"
echo "read me" | sudo -n tee "/etc/quality-data/$RUN.read" >/dev/null
touch "$MARK"

# --- 2 to 4. inside the sandbox -------------------------------------------------

cat >"$WORK/inside.sh" <<'EOF'
set -u
say() { if eval "$2" >/tmp/out 2>&1; then echo "PASS  $1"; else echo "FAIL  $1 ($(tail -1 /tmp/out))"; fi; }
not() { ! eval "$1"; }
compose() {
  docker compose --project-name "$RUN" --project-directory "$MODULE/current" \
    --file "$MODULE/current/compose.yml" --env-file "$MODULE/.env" "$@"
}
say "no new privileges" 'grep -qx "NoNewPrivs:[[:space:]]*1" /proc/self/status'
say "docker version reaches the daemon through its socket" 'docker version --format "{{.Server.Version}}"'
say "docker compose version finds the plugin" 'docker compose version'
say "docker --config in a private /tmp" 'c=$(mktemp -d) && docker --config "$c" version >/dev/null && rm -rf "$c"'
say "compose up --detach --no-build --wait" 'compose up --detach --no-build --remove-orphans --wait --wait-timeout 60'
say "compose exec -T" 'compose exec -T app true'
say "compose ps --all --format json" 'compose ps --all --format json | grep -q "\"Service\":\"app\""'
say "compose cp app:/data - into /var/lib/quality-data" \
  'compose exec -T app sh -c "echo one > /data/a" && compose cp app:/data - > "/var/lib/quality-data/$RUN/v.tar" && [ -s "/var/lib/quality-data/$RUN/v.tar" ]'
say "compose stop" 'compose stop'
say "docker run --volumes-from empties the stopped container's volume" \
  'id=$(compose ps --all --quiet app) && docker run --rm --name "$RUN-empty" --network none --user 0:0 --volumes-from "$id" --entrypoint find "$(docker inspect --format "{{.Image}}" "$id")" /data -mindepth 1 -delete'
say "compose cp - app:/ from /var/lib/quality-data" 'compose cp - app:/ < "/var/lib/quality-data/$RUN/v.tar"'
say "compose down" 'compose down --remove-orphans --timeout 1'
say "timeout and df -Pk" 'timeout 5 true && df -Pk /opt/quality-data /var/lib/quality-data'
for d in /var/lib/quality-data "$MODULE" /etc/quality-data/internal; do
  say "write, rename and remove in $d" \
    "mkdir -p '$d/$RUN.d' && echo x > '$d/$RUN.d/f.partial' && mv '$d/$RUN.d/f.partial' '$d/$RUN.d/f' && rm -r '$d/$RUN.d'"
done
say "a file in /etc/quality-data is read" 'grep -qx "read me" "/etc/quality-data/$RUN.read"'
for d in /etc /usr /var/lib /etc/quality-data; do
  say "writing in $d is refused" "not 'touch $d/$RUN.denied'"
done
say "/home is empty" '[ -z "$(ls -A /home 2>/dev/null)" ]'
say "the host's /tmp is not seen" "[ ! -e '/tmp/$RUN.host' ]"
EOF

sudo -n systemd-run --wait --pipe --quiet --collect --unit "$RUN" "${PROPS[@]}" \
  --setenv=PATH="$PATH" --setenv=RUN="$RUN" --setenv=MODULE="$MODULE" \
  "$(command -v bash)" -s <"$WORK/inside.sh" >"$WORK/inside.log" 2>&1 || true
while IFS= read -r line; do
  case "$line" in
  PASS\ \ *) record PASS "${line#PASS  }" ;;
  FAIL\ \ *) record FAIL "${line#FAIL  }" ;;
  esac
done <"$WORK/inside.log"
if ! grep -q '^PASS\|^FAIL' "$WORK/inside.log"; then
  record FAIL "the sandboxed service ran" "$(tail -3 "$WORK/inside.log" | tr '\n' ' ')"
fi

# --- Summary --------------------------------------------------------------

cat <<EOF

----- paste below into decision 0003, Verification -----
date: $(date -u +%Y-%m-%d)
$(systemctl --version | head -1); kernel $(uname -r)
$(docker version --format 'docker {{.Server.Version}}'); $(docker compose version)
image: $IMAGE
sandbox: ${PROPS[*]}
exposure: $EXPOSURE
$(printf '%s\n' "${RESULTS[@]}")
result: $( ((FAILED)) && echo FAIL || echo PASS)
---------------------------------------------------------
EOF
exit "$FAILED"
