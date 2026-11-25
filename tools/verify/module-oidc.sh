#!/usr/bin/env bash
# A module with OIDC clients, secrets and a database added, removed, added
# again and rotated against the core's real API (decision 0009,
# Verification V3). Local only: no account, no AWS, invented values.
#
# What is real: the agent's `qd module add|remove|rotate` logic, run by the
# example `module_offline` with `--api` (the signed index replaced by local
# packages and images, quality-data's updater/v1 called with the agent's
# certificate); the core of module-core.sh (core main's API image, its OIDC
# provider and PostgreSQL); an invented module `qsvbi` (module-oidc/) whose
# service is a small OIDC relying party, and `qsvtrino`, a stand-in provider
# of sql-query with `token-audience: trino`.
#
#   1. `add qsvtrino`, then `add qsvbi`: both clients registered (the core's
#      table), the secret files 0700/0400 owned by uid 1000, .env with the
#      QD_* lines and no secret, the role and its database;
#   2. the invented admin signs in through the people's client (ES256) and
#      gets bi_roles from its permissions (an admin holds every action);
#      the machine client gets a token for `trino` only; the apps menu shows
#      Analytics to the admin, who holds qsvbi:Author;
#   3. `remove qsvbi`: the clients disabled (sign-in and tokens refused),
#      the role NOLOGIN, the files kept;
#   4. `add qsvbi` again: the same secrets, sign-in works;
#   5. `rotate qsvbi`: new secrets, sign-in and the machine token work, and
#      what the core does with the previous machine secret is recorded;
#   6. every log of the agent, Traefik, the API and the module searched for
#      each secret: none found.
#
# Needs: docker, gh, curl, openssl, tar, the API image loaded
# (QD_API_IMAGE: gh run download <run of core main> -n image-api, then
# docker load), and the example built first, one build at a time:
#
#   capped cargo build -j 2 -p agent --example module_offline
#
# Run alone, under the machine lock, no build at the same time. Everything
# it starts is removed on exit, also on failure (see module-core.sh).
#
#   QD_API_IMAGE=qd-api:<sha> tools/verify/module-oidc.sh

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$ROOT_DIR/tools/verify"
BIN="${CARGO_TARGET_DIR:-$ROOT_DIR/target}/debug/examples/module_offline"
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
  for m in qsvbi qsvtrino; do
    docker ps -aq --filter "label=com.docker.compose.project=$m" | xargs -r docker rm -f -v >/dev/null 2>&1
    docker volume ls -q --filter "label=com.docker.compose.project=$m" | xargs -r docker volume rm >/dev/null 2>&1
  done
}
# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local rc=$?
  modules_down
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
record PASS "the core stand-in answers updater/v1" "$(core_agent_call GET /updater/v1/version)"

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
agent() { # ARGS: module_offline as the agent, its output kept for the log search
  "$BIN" --root "$R" "${AGENT[@]}" "$@" >>"$LOGS.agent" 2>&1
}
MOD="$R/opt/quality-data/qsvbi"
SECRETS="$MOD/secrets"
# What the module's service needs besides the agent's lines (the package
# never touches .env): where people reach the core, and its certificate.
mkdir -p "$MOD/config"
printf 'QS_PUBLIC_URL=%s\nQS_WEB_CA=%s\n' "$WEB" "$WORK/traefik/web.crt" >"$MOD/.env"
echo "invented settings" >"$MOD/config/settings.txt"

# --- 1. add ------------------------------------------------------------------
agent add qsvtrino "$(package qsvtrino)" "coordinator=$ALPINE_DIGEST" &&
  record PASS "add qsvtrino" || record FAIL "add qsvtrino" "$(tail -2 "$LOGS.agent")"
if agent add qsvbi "$(package qsvbi)" "web=$PYTHON"; then record PASS "add qsvbi"; else
  record FAIL "add qsvbi" "$(tail -3 "$LOGS.agent")"
  docker logs qs-verify-qsvbi-web 2>&1 | tail -5
fi
clients="$(core_sql "select client_id||':'||coalesce(module,'')||':'||(disabled_at is null)||':'||token_alg||':'||coalesce(array_to_string(audiences,','),'')
  from oidc_clients where client_id like 'qsvbi%' order by 1" | tr '\n' ' ')"
expect "1. both clients registered, enabled, of the module" \
  "qsvbi-machine:qsvbi:true:EdDSA:trino qsvbi:qsvbi:true:ES256: " "$clients"
expect "1. the secrets folder 0700, the agent's" "700 $(id -u)" "$(stat -c '%a %u' "$SECRETS")"
expect "1. each file 0400, owned by secrets.uid" \
  "database-password 400 1000 oidc-machine-secret 400 1000 oidc-people-secret 400 1000 secret-key 400 1000 " \
  "$(as_root "cd '$SECRETS' && stat -c '%n %a %u' *" | tr '\n' ' ')"
read_secret() { as_root "cat '$SECRETS/$1'"; }
PEOPLE1="$(read_secret oidc-people-secret)"
MACHINE1="$(read_secret oidc-machine-secret)"
DBPW="$(read_secret database-password)"
KEY1="$(read_secret secret-key)"
env_keys="$(grep -o '^QD_[A-Z_]*' "$MOD/.env" | tr '\n' ' ')"
expect "1. .env has the agent's QD_* lines" \
  "QD_SECRETS_DIR QD_DB_HOST QD_DB_PORT QD_DB_USER QD_DB_NAME QD_DB_CA_FILE QD_OIDC_ISSUER QD_OIDC_PEOPLE_CLIENT_ID QD_OIDC_MACHINE_CLIENT_ID " "$env_keys"
leak=no
for v in "$PEOPLE1" "$MACHINE1" "$DBPW" "$KEY1"; do grep -qF "$v" "$MOD/.env" && leak=yes; done
expect "1. .env holds no secret" no "$leak"
expect "1. the role and its database" "qsvbi:true:qsvbi" \
  "$(core_sql "select r.rolname||':'||r.rolcanlogin||':'||d.datname from pg_roles r join pg_database d on d.datdba = r.oid where r.rolname = 'qsvbi'")"

# --- 2. a person signs in; the machine token; the menu -------------------------
login() {
  rm -f "$WORK/jar"
  core_web /api/auth/login -o /dev/null -w '%{http_code}' -c "$WORK/jar" \
    -H 'content-type: application/json' -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASSWORD\"}"
}
signin() { core_web /bi/login -L -b "$WORK/jar" -c "$WORK/jar"; }
expect "2. the admin signs in to the core" 200 "$(login)"
claims="$(signin)"
roles="$(jq -c '.bi_roles' <<<"$claims" 2>/dev/null)"
expect "2. through the people's client, bi_roles from the permissions" '["admin","author"]' "$roles"
expect "2. the id_token's audience" qsvbi "$(jq -r '.aud' <<<"$claims" 2>/dev/null)"
machine="$(core_web /bi/machine)"
expect "2. the machine client's token is for trino only" '"trino"' "$(jq -c '.aud' <<<"$machine" 2>/dev/null)"
apps="$(core_web /api/settings/account/apps -b "$WORK/jar")"
entry="$(jq -r '.apps[] | select(.id == "qsvbi") | .url' <<<"$apps" 2>/dev/null)"
expect "2. the apps menu shows the entry to who holds qsvbi:Author" /bi/ "$entry"
[ -n "$entry" ] || echo "  apps: $apps"
record INFO "2. hidden without the action: not run here" \
  "the only account is the admin, who holds every action; core's C3 API tests cover it"

# --- 3. remove -----------------------------------------------------------------
token_with() { # CLIENT SECRET: the token endpoint's status, as a client would
  core_web /api/oauth/token -o /dev/null -w '%{http_code}' -u "$1:$2" -d grant_type=client_credentials
}
expect "3. before remove: the machine secret gets a token" 200 "$(token_with qsvbi-machine "$MACHINE1")"
agent remove qsvbi && record PASS "remove qsvbi" || record FAIL "remove qsvbi" "$(tail -2 "$LOGS.agent")"
expect "3. the clients disabled" "true true" \
  "$(core_sql "select disabled_at is not null from oidc_clients where client_id like 'qsvbi%'" | tr '\n' ' ' | sed 's/t/true/g; s/ $//')"
code="$(token_with qsvbi-machine "$MACHINE1")"
[ "$code" != 200 ] && record PASS "3. a token is refused" "$code" || record FAIL "3. a token is refused" "$code"
authz="$(core_web "/api/oauth/authorize?response_type=code&client_id=qsvbi&redirect_uri=$WEB/bi/cb&scope=openid&state=x&code_challenge=abc&code_challenge_method=S256" \
  -b "$WORK/jar" -o /dev/null -w '%{http_code} %{redirect_url}')"
[[ "$authz" != *"/bi/cb?code="* ]] && record PASS "3. sign-in is refused" "${authz%% *}" ||
  record FAIL "3. sign-in is refused" "$authz"
expect "3. the role NOLOGIN" f "$(core_sql "select rolcanlogin from pg_roles where rolname = 'qsvbi'")"
[ "$(read_secret oidc-people-secret)" = "$PEOPLE1" ] && [ -f "$MOD/.env" ] &&
  record PASS "3. the files kept" || record FAIL "3. the files kept"

# --- 4. add again ----------------------------------------------------------------
agent add qsvbi "$WORK/qsvbi.tar.gz" "web=$PYTHON" && record PASS "add qsvbi again" ||
  record FAIL "add qsvbi again" "$(tail -2 "$LOGS.agent")"
same=yes
for n in oidc-people-secret oidc-machine-secret database-password secret-key; do
  case $n in
    oidc-people-secret) [ "$(read_secret $n)" = "$PEOPLE1" ] || same=no ;;
    oidc-machine-secret) [ "$(read_secret $n)" = "$MACHINE1" ] || same=no ;;
    database-password) [ "$(read_secret $n)" = "$DBPW" ] || same=no ;;
    secret-key) [ "$(read_secret $n)" = "$KEY1" ] || same=no ;;
  esac
done
expect "4. every secret the same" yes "$same"
expect "4. sign-in works again" '["admin","author"]' "$(signin | jq -c '.bi_roles' 2>/dev/null)"
expect "4. the role LOGIN again" t "$(core_sql "select rolcanlogin from pg_roles where rolname = 'qsvbi'")"

# --- 5. rotate ----------------------------------------------------------------------
agent rotate qsvbi && record PASS "rotate qsvbi" || record FAIL "rotate qsvbi" "$(tail -2 "$LOGS.agent")"
PEOPLE2="$(read_secret oidc-people-secret)"
MACHINE2="$(read_secret oidc-machine-secret)"
[ "$PEOPLE2" != "$PEOPLE1" ] && [ "$MACHINE2" != "$MACHINE1" ] && [ "$(read_secret secret-key)" = "$KEY1" ] &&
  record PASS "5. new issued secrets, the module's own kept" || record FAIL "5. the rotated files"
[ -z "$(as_root "ls '$SECRETS' | grep -e '\\.next' -e '\\.tmp' || true")" ] &&
  record PASS "5. no .next or .tmp left" || record FAIL "5. files left aside"
expect "5. sign-in works with the new secret" '["admin","author"]' "$(signin | jq -c '.bi_roles' 2>/dev/null)"
expect "5. the new machine secret gets a token" 200 "$(token_with qsvbi-machine "$MACHINE2")"
record INFO "5. the previous machine secret after the rotation" \
  "token endpoint answered $(token_with qsvbi-machine "$MACHINE1") (the grace is at most one hour, ended by keep_previous: false)"
record INFO "5. the role's password" \
  "$( [ "$(read_secret database-password)" != "$DBPW" ] && echo changed || echo unchanged)"

# --- 6. no secret in any log --------------------------------------------------------
docker logs "$CN-api" >"$LOGS.api" 2>&1
docker logs "$CN-traefik" >"$LOGS.traefik" 2>&1
docker logs qs-verify-qsvbi-web >"$LOGS.module" 2>&1
docker logs "$CN-postgres" >"$LOGS.postgres" 2>&1
found=""
for v in "$PEOPLE1" "$MACHINE1" "$DBPW" "$KEY1" "$PEOPLE2" "$MACHINE2" "$(read_secret database-password)"; do
  for f in "$LOGS".*; do grep -qF "$v" "$f" && found="$found ${f##*.}"; done
done
expect "6. no secret in the logs of the agent, the API, Traefik, PostgreSQL and the module" "" "$found"

echo
echo "$QD_API_IMAGE, $(docker compose version --short), traefik v3.6, postgres:16-alpine"
printf '%s\n' "${RESULTS[@]}" | grep -c '^PASS' | xargs printf 'passed: %s\n'
exit "$FAILED"
