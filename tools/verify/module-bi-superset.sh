#!/usr/bin/env bash
# The bi-superset module added to a test installation and a person signed
# in to Superset (core decision 0089, step 8's check). Local only: no
# account, no AWS, invented values.
#
# What is real: the agent's `qd module add|remove` logic, run by the
# example `module_offline` with `--api`; the core of module-core.sh (core
# main's API image, its OIDC provider and PostgreSQL); bi-superset's own
# module.yaml and compose.yml from a checkout (BI_SUPERSET_DIR) and its image
# built from it; Valkey 9.0.8; `qsvtrino` (module-bi-superset/), a stand-in
# provider of sql-query with `token-audience: trino`. No Trino runs: this
# checks the install and the sign-in, not a query.
#
#   1. `add qsvtrino`, then `add bi-superset`: healthy (superset-init done,
#      Superset and Valkey up), both clients registered, the database made;
#   2. the core's admin signs in to the core, then to Superset through the
#      people's client: the sign-in ends on Superset's welcome page under
#      /bi, the username is the account id, the roles come from bi_roles;
#      the session cookie holds a session id only (sessions in Valkey);
#      Superset's own CSP with frame-ancestors 'self'; the apps menu shows
#      Analytics to the admin;
#   3. a person with no BI role is refused; a viewer only: not run here
#      (the core makes other accounts by e-mail; INFO);
#   4. `remove bi-superset`: sign-in refused, the files and database kept;
#   5. every log searched for each secret: none found.
#
# Needs: docker, gh, curl, openssl, jq, tar, the API image loaded
# (QD_API_IMAGE: gh run download <run of core main> -n image-api, then
# docker load), a checkout of government-data/bi-superset (BI_SUPERSET_DIR)
# and the example built first, one build at a time:
#
#   capped cargo build -j 2 -p agent --example module_offline
#
# Run alone, under the machine lock, no build at the same time. Everything
# it starts is removed on exit, also on failure.
#
#   QD_API_IMAGE=qd-api:<sha> BI_SUPERSET_DIR=../bi-superset tools/verify/module-bi-superset.sh

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$ROOT_DIR/tools/verify"
BIN="${CARGO_TARGET_DIR:-$ROOT_DIR/target}/debug/examples/module_offline"
BI="${BI_SUPERSET_DIR:?set BI_SUPERSET_DIR to a checkout of bi-superset}"
VALKEY="${VALKEY_IMAGE:-valkey/valkey:9.0.8-alpine}"
SUPERSET_TAG=qs-verify-bi-superset:local
DBNET=data-quality-db
WORK="$(mktemp -d)"
FAILED=0
RESULTS=()
LOGS="$WORK/logs"

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

modules_down() {
  for m in bi-superset qsvtrino; do
    docker ps -aq --filter "label=com.docker.compose.project=$m" | xargs -r docker rm -f -v >/dev/null 2>&1
    docker volume ls -q --filter "label=com.docker.compose.project=$m" | xargs -r docker volume rm >/dev/null 2>&1
    docker network ls -q --filter "label=com.docker.compose.project=$m" | xargs -r docker network rm >/dev/null 2>&1
  done
}
# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local rc=$?
  modules_down
  docker network disconnect -f "$DBNET" "$CN-postgres" >/dev/null 2>&1
  core_down
  docker network rm "$DBNET" >/dev/null 2>&1
  docker rmi "$SUPERSET_TAG" >/dev/null 2>&1
  rm -rf -- "${WORK:?}"
  exit "$rc"
}
trap cleanup EXIT

[ -x "$BIN" ] || { echo "build the example first: $BIN"; exit 2; }
[ -f "$BI/module.yaml" ] && [ -f "$BI/compose.yml" ] || { echo "no bi-superset checkout at $BI"; exit 2; }
for m in bi-superset qsvtrino; do
  [ -z "$(docker ps -aq --filter "label=com.docker.compose.project=$m")" ] || { echo "a project $m exists"; exit 2; }
done
docker network inspect "$DBNET" >/dev/null 2>&1 && { echo "the network $DBNET exists"; exit 2; }
core_preflight || exit 2
docker build -q -f "$BI/image/Dockerfile" -t "$SUPERSET_TAG" "$BI" >/dev/null ||
  { record FAIL "bi-superset's image builds"; exit 1; }
docker pull -q "$VALKEY" >/dev/null || { record FAIL "Valkey's image"; exit 1; }
core_up || { record FAIL "the core stand-in"; exit 1; }
record PASS "the core stand-in answers updater/v1" "$(core_agent_call GET /updater/v1/version)"
# The core's package puts PostgreSQL on data-quality-db by its server name
# (decision 0009, point 5); module-core.sh's stand-in has no such network.
docker network create "$DBNET" >/dev/null
docker network connect --alias "$CN-postgres" "$DBNET" "$CN-postgres"

ALPINE_DIGEST="$(docker image inspect "$ALPINE" --format '{{index .RepoDigests 0}}')"
package() { # MODULE DIR...: the package of a module's files
  local m=$1 d="$WORK/pkg-$1"
  shift
  mkdir -p "$d"
  cp "$@" "$d/"
  tar -C "$d" -czf "$WORK/$m.tar.gz" .
  echo "$WORK/$m.tar.gz"
}
mapfile -t AGENT < <(core_agent_args)
agent() { "$BIN" --root "$R" "${AGENT[@]}" "$@" >>"$LOGS.agent" 2>&1; }
MOD="$R/opt/quality-data/bi-superset"
SECRETS="$MOD/secrets"
# What a test installation needs besides the agent's lines: PostgreSQL has
# no TLS in module-core.sh, and the web entry point's certificate is its own.
mkdir -p "$MOD"
printf 'QD_DB_SSLMODE=disable\nQD_WEB_CA_FILE=%s\n' "$WORK/traefik/web.crt" >"$MOD/.env"

# --- 1. add ---------------------------------------------------------------------
cp "$HERE/module-bi-superset/qsvtrino.module.yaml" "$WORK/module.yaml"
cp "$HERE/module-bi-superset/qsvtrino.compose.yml" "$WORK/compose.yml"
agent add qsvtrino "$(package qsvtrino "$WORK/module.yaml" "$WORK/compose.yml")" "coordinator=$ALPINE_DIGEST" &&
  record PASS "1. add qsvtrino" || record FAIL "1. add qsvtrino" "$(tail -2 "$LOGS.agent")"
if agent add bi-superset "$(package bi-superset "$BI/module.yaml" "$BI/compose.yml")" \
  "superset=$SUPERSET_TAG" "valkey=$VALKEY"; then record PASS "1. add bi-superset, healthy"; else
  record FAIL "1. add bi-superset" "$(tail -3 "$LOGS.agent")"
  for c in $(docker ps -aq --filter label=com.docker.compose.project=bi-superset); do
    n="$(docker inspect -f '{{.Name}}' "$c")"
    docker logs --tail 8 "$c" 2>&1 | while IFS= read -r l; do printf '  %s: %s\n' "${n#/}" "$l"; done
  done
fi
expect "1. superset-init completed" "exited 0" \
  "$(docker ps -a --filter label=com.docker.compose.service=superset-init --format '{{.State}}' | head -1) $(docker inspect -f '{{.State.ExitCode}}' "$(docker ps -aq --filter label=com.docker.compose.service=superset-init | head -1)" 2>/dev/null)"
expect "1. both clients registered and enabled" "bi-superset-machine:true:trino bi-superset:true: " \
  "$(core_sql "select client_id||':'||(disabled_at is null)||':'||coalesce(array_to_string(audiences,','),'')
     from oidc_clients where client_id like 'bi-superset%' order by 1" | tr '\n' ' ')"
expect "1. the role and its database" "bi_superset:true:bi_superset" \
  "$(core_sql "select r.rolname||':'||r.rolcanlogin||':'||d.datname from pg_roles r join pg_database d on d.datdba = r.oid where r.rolname = 'bi_superset'")"

# --- 2. the admin signs in to Superset -------------------------------------------------
login() {
  rm -f "$WORK/jar"
  core_web /api/auth/login -o /dev/null -w '%{http_code}' -c "$WORK/jar" \
    -H 'content-type: application/json' -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASSWORD\"}"
}
bi() { core_web "/bi$1" -b "$WORK/jar" -c "$WORK/jar" "${@:2}"; }
expect "2. the admin signs in to the core" 200 "$(login)"
expect "2. Superset's sign-in through the people's client ends under /bi" "200 $WEB/bi/superset/welcome/" \
  "$(bi /login/qd -L -o /dev/null -D "$WORK/signin.headers" -w '%{http_code} %{url_effective}')"
ADMIN_ID="$(core_sql "select id from admin_users where email = '$ADMIN_EMAIL'")"
expect "2. the username is the account id" "$ADMIN_ID" "$(bi /api/v1/me/ | jq -r '.result.username // "none"')"
expect "2. the roles from bi_roles (an admin holds every action)" "Admin,QD Author,QD Viewer" \
  "$(bi /api/v1/me/roles/ | jq -r '[.result.roles // {} | keys[]] | sort | join(",")')"
cookie="$(awk '$6 == "session" { print $7 }' "$WORK/jar")"
case "$cookie" in
  *eyJ*) record FAIL "2. the session cookie holds a session id only" "a JWT in it" ;;
  "") record FAIL "2. the session cookie holds a session id only" "no cookie" ;;
  *) record PASS "2. the session cookie holds a session id only" "${#cookie} characters, no JWT" ;;
esac
keys="$(docker exec "$(docker ps -q --filter label=com.docker.compose.service=valkey | head -1)" sh -c \
  'valkey-cli -a "$(cat /run/secrets/valkey-password)" --no-auth-warning -n 3 dbsize' 2>/dev/null)"
[ "${keys:-0}" -gt 0 ] 2>/dev/null && record PASS "2. the session is in Valkey" "$keys keys" ||
  record FAIL "2. the session is in Valkey" "${keys:-none}"
csp="$(bi /login/ -o /dev/null -D - | tr -d '\r' | sed -n 's/^content-security-policy: //Ip' | head -1)"
case "$csp" in
  *"frame-ancestors 'self'"*"default-src 'self'"* | *"default-src 'self'"*"frame-ancestors 'self'"*)
    record PASS "2. Superset's own CSP, with frame-ancestors 'self'" ;;
  *) record FAIL "2. Superset's own CSP, with frame-ancestors 'self'" "${csp:0:200}" ;;
esac
entry="$(core_web /api/settings/account/apps -b "$WORK/jar" | jq -r '.apps[] | select(.id == "bi-superset") | .url' 2>/dev/null)"
expect "2. the apps menu shows Analytics to who holds bi-superset:Author" /bi/ "$entry"

# --- 3. who may not ------------------------------------------------------------------
rm -f "$WORK/jar"
expect "3. with no session of the core, Superset's sign-in does not end on its welcome page" no \
  "$(bi /login/qd -L -o /dev/null -w '%{url_effective}' | grep -q '/bi/superset/welcome/' && echo yes || echo no)"
record INFO "3. a viewer only, a person with no BI role: not run here" \
  "the core makes accounts by e-mail; the role mapping and the refusal are core 0089's V1"

# The module's own logs, for step 5, before remove stops it.
for c in $(docker ps -aq --filter label=com.docker.compose.project=bi-superset); do
  docker logs "$c" >>"$LOGS.module" 2>&1
done

# --- 4. remove ----------------------------------------------------------------------------
agent remove bi-superset && record PASS "4. remove bi-superset" || record FAIL "4. remove bi-superset" "$(tail -2 "$LOGS.agent")"
expect "4. the clients disabled" "true true" \
  "$(core_sql "select disabled_at is not null from oidc_clients where client_id like 'bi-superset%'" | tr '\n' ' ' | sed 's/t/true/g; s/ $//')"
expect "4. the database kept, its role NOLOGIN" "bi_superset:false" \
  "$(core_sql "select d.datname||':'||r.rolcanlogin from pg_database d join pg_roles r on d.datdba = r.oid where d.datname = 'bi_superset'")"
[ -f "$MOD/.env" ] && [ -n "$(as_root "ls '$SECRETS'")" ] && record PASS "4. the files kept" || record FAIL "4. the files kept"

# --- 5. no secret in any log ----------------------------------------------------------------
docker logs "$CN-api" >"$LOGS.api" 2>&1
docker logs "$CN-traefik" >"$LOGS.traefik" 2>&1
docker logs "$CN-postgres" >"$LOGS.postgres" 2>&1
found=""
for n in oidc-people-secret oidc-machine-secret database-password secret-key valkey-password; do
  v="$(as_root "cat '$SECRETS/$n'")"
  [ -n "$v" ] || continue
  for f in "$LOGS".* "$WORK"/*.log; do [ -f "$f" ] && grep -qF "$v" "$f" && found="$found $n:${f##*/}"; done
done
expect "5. no secret in the logs of the agent, the API, Traefik, PostgreSQL and the module" "" "$found"

echo
echo "$QD_API_IMAGE, bi-superset $(git -C "$BI" rev-parse --short HEAD), $VALKEY, $(docker compose version --short)"
printf '%s\n' "${RESULTS[@]}" | grep -c '^PASS' | xargs printf 'passed: %s\n'
exit "$FAILED"
