#!/usr/bin/env bash
# A module purged against the core's real API (decision 0009, Verification
# V4). Local only: no account, no AWS, invented values.
#
# What is real: as module-oidc.sh (the agent's `qd module add|purge` logic
# run by `module_offline --api`, the core of module-core.sh, the invented
# modules of module-oidc/).
#
#   1. `purge` with the core stopped: refused, nothing deleted (the module
#      is removed first, as `remove`);
#   2. a critical key made unreadable: the backup fails and nothing is
#      deleted;
#   3. a crash injected at `delete` (Traefik refuses the agent's DELETE):
#      the purge stops there, its backup complete, nothing of the core's
#      deleted; `purge` again resumes and ends;
#   4. after it: the backup folder complete, its dump restorable into a
#      throwaway server, the volume's archive holding what the module wrote
#      (copied from a module that is down, through `docker compose create
#      --no-recreate` and `cp`), the clients gone, the database and the role
#      dropped, the volumes and the module's folder gone, and the audit
#      events in order.
#
# Needs and runs as module-oidc.sh:
#
#   QD_API_IMAGE=qd-api:<sha> tools/verify/module-purge.sh

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$ROOT_DIR/tools/verify"
BIN="${CARGO_TARGET_DIR:-$ROOT_DIR/target}/debug/examples/module_offline"
WORK="$(mktemp -d)"
FAILED=0
RESULTS=()

record() { # record PASS|FAIL|INFO name [detail]
  local line="$1  $2${3:+ ($3)}"
  echo "$line"
  RESULTS+=("$line")
  if [[ "$1" == FAIL ]]; then FAILED=1; fi
}
expect() { # expect name want got
  if [ "$2" = "$3" ]; then record PASS "$1" "$3"; else record FAIL "$1" "want $2, got $3"; fi
}

# shellcheck source=tools/verify/module-core.sh
. "$HERE/module-core.sh"

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local rc=$?
  for m in qsvbi qsvtrino; do
    docker ps -aq --filter "label=com.docker.compose.project=$m" | xargs -r docker rm -f -v >/dev/null 2>&1
    docker volume ls -q --filter "label=com.docker.compose.project=$m" | xargs -r docker volume rm >/dev/null 2>&1
  done
  docker rm -f "$CN-restore" >/dev/null 2>&1
  core_down
  rm -rf -- "${WORK:?}"
  exit "$rc"
}
trap cleanup EXIT

[ -x "$BIN" ] || { echo "build the example first: $BIN"; exit 2; }
for m in qsvbi qsvtrino; do
  [ -z "$(docker ps -aq --filter "label=com.docker.compose.project=$m")" ] || { echo "a project $m exists"; exit 2; }
done
core_preflight || exit 2
core_up || { record FAIL "the core stand-in"; exit 1; }

PYTHON="$(docker image inspect python:3.12-slim --format '{{index .RepoDigests 0}}')"
ALPINE_DIGEST="$(docker image inspect "$ALPINE" --format '{{index .RepoDigests 0}}')"
package() { # MODULE: its package from module-oidc/
  local d="$WORK/pkg-$1"
  mkdir -p "$d"
  cp "$HERE/module-oidc/$1.module.yaml" "$d/module.yaml"
  cp "$HERE/module-oidc/$1.compose.yml" "$d/compose.yml"
  [ "$1" = qsvbi ] && cp "$HERE/module-oidc/rp.py" "$d/rp.py"
  tar -C "$d" -czf "$WORK/$1.tar.gz" .
  echo "$WORK/$1.tar.gz"
}
mapfile -t AGENT < <(core_agent_args)
agent() { "$BIN" --root "$R" "${AGENT[@]}" "$@" >"$WORK/agent.out" 2>&1; }
MOD="$R/opt/quality-data/qsvbi"
BACKUPS="$R/var/lib/quality-data/backups/purge"
mkdir -p "$MOD/config"
printf 'QS_PUBLIC_URL=%s\nQS_WEB_CA=%s\n' "$WEB" "$WORK/traefik/web.crt" >"$MOD/.env"
echo "invented settings" >"$MOD/config/settings.txt"
agent add qsvtrino "$(package qsvtrino)" "coordinator=$ALPINE_DIGEST" || record FAIL "add qsvtrino" "$(tail -2 "$WORK/agent.out")"
agent add qsvbi "$(package qsvbi)" "web=$PYTHON" && record PASS "add qsvbi" ||
  { record FAIL "add qsvbi" "$(tail -3 "$WORK/agent.out")"; exit 1; }
# What the module keeps: a file in its volume and a table in its database.
docker run --rm -v qsvbi_data:/data "$ALPINE" sh -c 'echo qs-verify volume data >/data/marker'
docker exec "$CN-postgres" psql -U postgres -d qsvbi -XAtqc \
  "create table kept (n int); insert into kept values (1), (2), (3)"

# What exists of the module, as one line.
present() {
  local clients role db volume folder
  clients="$(core_sql "select count(*) from oidc_clients where module = 'qsvbi'")"
  role="$(core_sql "select count(*) from pg_roles where rolname = 'qsvbi'")"
  db="$(core_sql "select count(*) from pg_database where datname = 'qsvbi'")"
  volume="$(docker volume ls -q --filter label=com.docker.compose.project=qsvbi | wc -l)"
  folder="$([ -d "$MOD" ] && echo 1 || echo 0)"
  echo "clients=$clients role=$role db=$db volumes=$volume folder=$folder"
}
ALL="clients=2 role=1 db=1 volumes=1 folder=1"
expect "before: everything of the module is there" "$ALL" "$(present)"

# --- 1. the core stopped ------------------------------------------------------
docker stop "$CN-api" >/dev/null
if agent purge qsvbi --confirm qsvbi; then record FAIL "1. purge with the core stopped is refused"; else
  record PASS "1. purge with the core stopped is refused" "$(grep -o 'not purged[^(]*' "$WORK/agent.out" | head -1)"
fi
docker start "$CN-api" >/dev/null
for _ in $(seq 30); do
  [ "$(core_agent_call GET /updater/v1/version -o /dev/null -w '%{http_code}')" = 200 ] && break
  sleep 1
done
expect "1. nothing deleted (the module removed first)" "$ALL" "$(present)"
expect "1. no backup" 0 "$(find "$BACKUPS" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"

# --- 2. a critical piece that cannot be backed up -----------------------------
chmod 000 "$MOD/config/settings.txt"
if agent purge qsvbi --confirm qsvbi; then record FAIL "2. an unreadable key stops the purge"; else
  record PASS "2. an unreadable key stops the purge" "$(grep -o 'the purge.s backup failed, nothing was deleted' "$WORK/agent.out" | head -1)"
fi
chmod 644 "$MOD/config/settings.txt"
expect "2. nothing deleted" "$ALL" "$(present)"
expect "2. no backup left" 0 "$(find "$BACKUPS" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"

# --- 3. a crash at delete, then resumed ------------------------------------------
cat >"$CORE/internal/qs-verify-crash.yml" <<EOF
http:
  routers:
    qs-verify-crash:
      rule: "Host(\`$INTERNAL_DOMAIN\`) && Method(\`DELETE\`) && PathPrefix(\`/api/updater/v1/modules/\`)"
      entryPoints: [internal]
      priority: 1000
      tls:
        options: machines
      service: qs-verify-nowhere
  services:
    qs-verify-nowhere:
      loadBalancer:
        servers: [{ url: "http://127.0.0.1:9" }]
EOF
sleep 3
if agent purge qsvbi --confirm qsvbi; then record FAIL "3. the purge stops at delete"; else
  record PASS "3. the purge stops at delete" "$(grep -o 'stopped at [a-z]*' "$WORK/agent.out" | head -1)"
fi
expect "3. its progress file" '"purging" "delete"' \
  "$(jq -r '"\"" + .operation + "\" \"" + .step + "\""' "$R/var/lib/quality-data/updater/modules/qsvbi.json" 2>/dev/null)"
expect "3. nothing of the core's deleted yet" "$ALL" "$(present)"
expect "3. the backup complete" 1 "$(find "$BACKUPS" -mindepth 2 -maxdepth 2 -name complete | wc -l)"
if "$BIN" --root "$R" "${AGENT[@]}" add qsvbi "$WORK/qsvbi.tar.gz" "web=$PYTHON" >"$WORK/add.out" 2>&1; then
  record FAIL "3. add is refused while a purge is under way"
else
  record PASS "3. add is refused while a purge is under way" "$(grep -o 'a purge of qsvbi is under way' "$WORK/add.out")"
fi
rm -f "$CORE/internal/qs-verify-crash.yml"
sleep 3
agent purge qsvbi --confirm qsvbi && record PASS "3. purge again resumes and ends" ||
  record FAIL "3. purge again" "$(tail -2 "$WORK/agent.out")"
expect "3. one backup only (it resumed after the backup)" 1 "$(find "$BACKUPS" -mindepth 1 -maxdepth 1 | wc -l)"

# --- 4. what the purge left ----------------------------------------------------------
expect "4. the clients, database, role, volumes and folder gone" \
  "clients=0 role=0 db=0 volumes=0 folder=0" "$(present)"
B="$(find "$BACKUPS" -mindepth 1 -maxdepth 1 -type d | head -1)"
expect "4. the backup folder 0700" 700 "$(stat -c '%a' "$B")"
expect "4. its pieces" \
  "complete modules/qsvbi/keys/config/settings.txt modules/qsvbi/keys/secrets/database-password modules/qsvbi/keys/secrets/oidc-machine-secret modules/qsvbi/keys/secrets/oidc-people-secret modules/qsvbi/keys/secrets/secret-key modules/qsvbi/volumes/data.tar qsvbi.dump " \
  "$(cd "$B" && find . -type f | sed 's|^\./||' | sort | tr '\n' ' ')"
expect "4. the volume, copied from a module that is down (compose create --no-recreate, cp)" \
  "qs-verify volume data" "$(tar -xOf "$B/modules/qsvbi/volumes/data.tar" data/marker 2>/dev/null)"
docker run -d --name "$CN-restore" -e POSTGRES_PASSWORD="qs-verify-$RANDOM" postgres:16-alpine >/dev/null
for _ in $(seq 30); do docker exec "$CN-restore" pg_isready -U postgres >/dev/null 2>&1 && break; sleep 1; done
sleep 2
docker exec "$CN-restore" createdb -U postgres qsvbi
docker exec -i "$CN-restore" pg_restore -U postgres -d qsvbi --no-owner <"$B/qsvbi.dump" >/dev/null 2>&1
expect "4. its dump restores into a throwaway server" 3 \
  "$(docker exec "$CN-restore" psql -U postgres -d qsvbi -XAtc 'select count(*) from kept' 2>&1)"
core_sql "select chain_seq||' '||coalesce(event_type,'')||' '||coalesce(action,'')||' '||coalesce(resource,'')
  from admin_audit_events order by chain_seq" >"$WORK/audit.txt"
sed 's/^/  audit: /' "$WORK/audit.txt"
events="$(awk '$2 ~ /^module_/ && $NF == "module/qsvbi" {print $2}' "$WORK/audit.txt" | tr '\n' ' ')"
expect "4. the core's audit chain, in order" \
  "module_client_registered module_client_registered module_added module_purge_started module_client_deleted module_client_deleted module_purged " \
  "$events"
# `remove` ran with the core stopped (1.): its event waits in the outbox,
# which the service sends when it starts.
expect "4. ModuleRemoved waits in the agent's audit outbox" ModuleRemoved \
  "$(jq -r .event "$R/var/lib/quality-data/updater/audit-outbox.jsonl" 2>/dev/null | tr '\n' ' ' | xargs)"

echo
echo "$QD_API_IMAGE, $(docker compose version --short), traefik v3.6, postgres:16-alpine"
printf '%s\n' "${RESULTS[@]}" | grep -c '^PASS' | xargs printf 'passed: %s\n'
exit "$FAILED"
