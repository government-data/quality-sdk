#!/usr/bin/env bash
# Checks how PostgreSQL behaves where the agent creates a module's role and
# databases (decision 0009, point 5, Verification V2), before the agent's
# PostgreSQL adapter builds on it. Local only: throwaway containers, a
# throwaway CA, invented values, no account.
#
# A `qs-verify-…` server of the core's image with TLS (a certificate for
# its name from a throwaway CA) and the core's pg_hba.conf rules (nothing
# over TCP without TLS, every role over TLS with its password; the lines
# for the queue's and the metrics' certificates are left out), on a
# network of its own; a client container of the same image reaches it by
# that name, as a module's service does on `data-quality-db`. Every
# statement goes on psql's standard input, as the adapter sends it, run
# inside the server's container as the superuser on the local socket.
#
#   1. A role whose password is set from a SCRAM-SHA-256 verifier computed
#      here (not by the server) signs in over TLS with `verify-full`; a
#      wrong password and a connection without TLS are refused.
#   2. With `log_statement=all`, the server's log holds the verifier of a
#      failing `ALTER ROLE` and never the clear password; a failing
#      `ALTER ROLE … PASSWORD '<clear>'` puts the clear password in the
#      log (why the agent sends a verifier).
#   3. Before `REVOKE CONNECT … FROM PUBLIC` on another database, the
#      module's role connects to it (the reason for the core's step C5);
#      after it, it cannot; it still connects to its own.
#   4. `NOLOGIN` refuses a new session; `pg_terminate_backend` ends the
#      sessions open; `LOGIN` again lets it in with the same password.
#   5. `DROP DATABASE … WITH (FORCE)` with a session open, then `DROP ROLE`.
#   6. Every statement of creation run twice: the second run changes
#      nothing and does not fail.
#
# Containers, network and files are removed on exit, also on failure. Run
# alone, under the machine lock: no build at the same time (AGENTS.md).
#
#   tools/verify/module-database.sh
#
# Environment:
#   QD_POSTGRES_IMAGE   the image (default postgres:16-alpine, as the core)
#
# Needs: docker, openssl, python3.

set -euo pipefail

IMAGE="${QD_POSTGRES_IMAGE:-postgres:16-alpine}"
RUN="qs-verify-db-$(date -u +%Y%m%dt%H%M%Sz)-$RANDOM"
SERVER_NAME="qs-verify-postgres.internal"
WORK="$(mktemp -d)"
FAILED=0
RESULTS=()
ROLE=qs_verify_mod
DB=qs_verify_mod
PASSWORD="qs-verify-pw-$RANDOM$RANDOM$RANDOM"

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
  docker rm -f -v "$RUN" "$RUN-hold" >/dev/null 2>&1
  docker network rm "$RUN" >/dev/null 2>&1
  if docker ps -a --format '{{.Names}}' | grep -q "^$RUN"; then
    echo "WARN: remove the containers $RUN* by hand"
    rc=1
  fi
  rm -rf "$WORK"
  exit "$rc"
}
trap cleanup EXIT

command -v openssl >/dev/null || die "openssl is not installed"
command -v python3 >/dev/null || die "python3 is not installed"

# --- the server: TLS from a throwaway CA, the core's rules --------------------

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
  -subj "/CN=qs-verify CA" -keyout "$WORK/ca.key" -out "$WORK/ca.crt" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj "/CN=$SERVER_NAME" \
  -keyout "$WORK/server.key" -out "$WORK/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:%s\n' "$SERVER_NAME" >"$WORK/san.ext"
openssl x509 -req -in "$WORK/server.csr" -CA "$WORK/ca.crt" -CAkey "$WORK/ca.key" \
  -CAcreateserial -days 1 -extfile "$WORK/san.ext" -out "$WORK/server.crt" 2>/dev/null
cat >"$WORK/pg_hba.conf" <<'EOF'
local      all  all       trust
hostnossl  all  all  all  reject
hostssl    all  all  all  scram-sha-256
EOF
chmod 0644 "$WORK"/*
docker network create "$RUN" >/dev/null
docker run -d --name "$RUN" --network "$RUN" --network-alias "$SERVER_NAME" \
  -e POSTGRES_USER=qd -e POSTGRES_PASSWORD="qs-verify-super-$RANDOM" -e POSTGRES_DB=knime \
  -v "$WORK:/qs:ro" --entrypoint sh "$IMAGE" -c '
    install -o postgres -m 0600 /qs/server.key /tmp/server.key &&
    install -o postgres -m 0644 /qs/server.crt /qs/pg_hba.conf /tmp/ &&
    exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/server.crt \
      -c ssl_key_file=/tmp/server.key -c hba_file=/tmp/pg_hba.conf \
      -c log_statement=all -c log_min_error_statement=error' >/dev/null
for _ in $(seq 60); do
  if docker exec "$RUN" pg_isready -U qd -h 127.0.0.1 >/dev/null 2>&1 &&
    docker exec "$RUN" psql -U qd -d knime -Atc 'select 1' >/dev/null 2>&1; then break; fi
  sleep 1
done
docker exec "$RUN" psql -U qd -d knime -Atc 'select 1' >/dev/null ||
  die "the server did not start: $(docker logs "$RUN" 2>&1 | tail -5)"

# As the adapter: the script on standard input, as the superuser, locally.
admin() { docker exec -i "$RUN" psql -U qd -d postgres -v ON_ERROR_STOP=1 -At "$@"; }
# As a module's service: over TLS by name, verify-full against the CA.
as_module() { # as_module database password [sslmode]
  docker run --rm --network "$RUN" -v "$WORK/ca.crt:/ca.crt:ro" -e PGPASSWORD="$2" \
    -e PGCONNECT_TIMEOUT=5 "$IMAGE" psql -Atc 'select current_user' \
    "host=$SERVER_NAME dbname=$1 user=$ROLE sslmode=${3:-verify-full} sslrootcert=/ca.crt"
}

# The verifier as the agent computes it: SCRAM-SHA-256, 4096 iterations, a
# random salt (RFC 5802, 7677; the format of pg_authid.rolpassword).
VERIFIER="$(
  PW="$PASSWORD" python3 -I -c '
import base64, hashlib, hmac, os
pw = os.environ["PW"].encode(); salt = os.urandom(16); it = 4096
salted = hashlib.pbkdf2_hmac("sha256", pw, salt, it)
client = hmac.new(salted, b"Client Key", "sha256").digest()
server = hmac.new(salted, b"Server Key", "sha256").digest()
b = lambda x: base64.b64encode(x).decode()
print(f"SCRAM-SHA-256${it}:{b(salt)}${b(hashlib.sha256(client).digest())}:{b(server)}")'
)"

# What the agent sends, every statement safe to run twice.
create_sql() {
  cat <<EOF
SELECT 'CREATE ROLE $ROLE' WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '$ROLE')\gexec
ALTER ROLE $ROLE LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT CONNECTION LIMIT 50;
ALTER ROLE $ROLE PASSWORD '$VERIFIER';
SELECT 'CREATE DATABASE $DB OWNER $ROLE' WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '$DB')\gexec
REVOKE CONNECT, TEMPORARY ON DATABASE $DB FROM PUBLIC;
GRANT CONNECT ON DATABASE $DB TO $ROLE;
EOF
}
state() {
  admin -c "select r.rolname, r.rolcanlogin, r.rolconnlimit, r.rolpassword, d.datname,
    pg_get_userbyid(d.datdba), d.datacl::text from pg_authid r, pg_database d
    where r.rolname = '$ROLE' and d.datname = '$DB'"
}

# --- 1. a verifier computed here signs in over TLS ----------------------------

create_sql | admin >"$WORK/create1.log" 2>&1 || die "create: $(tail -1 "$WORK/create1.log")"
first="$(state)"
if [[ "$(as_module "$DB" "$PASSWORD" 2>&1)" == "$ROLE" ]]; then
  record PASS "a role set from a SCRAM-SHA-256 verifier signs in over TLS with verify-full"
else
  record FAIL "a role set from a SCRAM-SHA-256 verifier signs in over TLS with verify-full" \
    "$(as_module "$DB" "$PASSWORD" 2>&1 | tail -1)"
fi
if as_module "$DB" "wrong-$PASSWORD" >/dev/null 2>&1 || as_module "$DB" "$PASSWORD" disable >/dev/null 2>&1; then
  record FAIL "a wrong password and a connection without TLS are refused"
else
  record PASS "a wrong password and a connection without TLS are refused"
fi

# --- 6. every statement twice changes nothing ---------------------------------

if create_sql | admin >"$WORK/create2.log" 2>&1; then
  second="$(state)"
  if [[ "$first" == "$second" ]]; then
    record PASS "every statement of creation run twice changes nothing"
  else
    record FAIL "every statement of creation run twice changes nothing" "the state differs"
  fi
else
  record FAIL "every statement of creation run twice changes nothing" "$(tail -1 "$WORK/create2.log")"
fi

# --- 2. the server's log holds no clear password ------------------------------

echo "ALTER ROLE qs_verify_absent PASSWORD '$VERIFIER';" | admin >/dev/null 2>&1 || true
logs="$(docker logs "$RUN" 2>&1)"
if grep -qF "$PASSWORD" <<<"$logs"; then
  record FAIL "with log_statement=all and a failing ALTER ROLE the log holds no clear password"
elif grep -qF "qs_verify_absent" <<<"$logs"; then
  record PASS "with log_statement=all and a failing ALTER ROLE the log holds no clear password" \
    "it holds the statement, with the verifier"
else
  record FAIL "with log_statement=all and a failing ALTER ROLE the log holds no clear password" \
    "the statement is not in the log: the check proves nothing"
fi
CLEAR="qs-verify-clear-$RANDOM$RANDOM"
echo "ALTER ROLE qs_verify_absent PASSWORD '$CLEAR';" | admin >/dev/null 2>&1 || true
if docker logs "$RUN" 2>&1 | grep -qF "$CLEAR"; then
  record PASS "a failing ALTER ROLE with a clear password puts it in the log" "why a verifier"
else
  record FAIL "a failing ALTER ROLE with a clear password puts it in the log" "it does not"
fi

# --- 3. CONNECT for PUBLIC on another database --------------------------------

if [[ "$(as_module knime "$PASSWORD" 2>&1)" == "$ROLE" ]]; then
  record PASS "before REVOKE CONNECT FROM PUBLIC the module's role connects to another database"
else
  record FAIL "before REVOKE CONNECT FROM PUBLIC the module's role connects to another database"
fi
admin -c 'REVOKE CONNECT, TEMPORARY ON DATABASE knime FROM PUBLIC' >/dev/null
if as_module knime "$PASSWORD" >/dev/null 2>&1; then
  record FAIL "after it the module's role cannot connect to it"
elif [[ "$(as_module "$DB" "$PASSWORD" 2>&1)" == "$ROLE" ]]; then
  record PASS "after it the module's role cannot connect to it" "it still connects to its own"
else
  record FAIL "after it the module's role cannot connect to it" "nor to its own"
fi

# --- 4. NOLOGIN, pg_terminate_backend, LOGIN ----------------------------------

hold() { # a session of the module's role kept open
  docker run -d --name "$RUN-hold" --network "$RUN" -v "$WORK/ca.crt:/ca.crt:ro" \
    -e PGPASSWORD="$PASSWORD" "$IMAGE" psql -Atc 'select pg_sleep(120)' \
    "host=$SERVER_NAME dbname=$DB user=$ROLE sslmode=verify-full sslrootcert=/ca.crt" >/dev/null
  for _ in $(seq 20); do
    [[ "$(admin -c "select count(*) from pg_stat_activity where usename = '$ROLE'")" -gt 0 ]] && return
    sleep 0.5
  done
}
sessions() { admin -c "select count(*) from pg_stat_activity where usename = '$ROLE'"; }
hold
open_before="$(sessions)"
admin <<EOF >/dev/null
ALTER ROLE $ROLE NOLOGIN;
SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = '$ROLE';
EOF
sleep 1
if [[ "$open_before" -gt 0 && "$(sessions)" == 0 ]]; then
  record PASS "pg_terminate_backend ends the role's sessions" "$open_before ended"
else
  record FAIL "pg_terminate_backend ends the role's sessions" "before $open_before, after $(sessions)"
fi
if as_module "$DB" "$PASSWORD" >/dev/null 2>&1; then
  record FAIL "NOLOGIN refuses a new session"
else
  record PASS "NOLOGIN refuses a new session"
fi
docker rm -f "$RUN-hold" >/dev/null
admin -c "ALTER ROLE $ROLE LOGIN" >/dev/null
if [[ "$(as_module "$DB" "$PASSWORD" 2>&1)" == "$ROLE" ]]; then
  record PASS "LOGIN again lets it in with the same password"
else
  record FAIL "LOGIN again lets it in with the same password"
fi

# --- 5. DROP DATABASE WITH (FORCE) with a session open, then DROP ROLE --------

hold
if admin -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)" >"$WORK/drop.log" 2>&1 &&
  admin -c "DROP ROLE IF EXISTS $ROLE" >>"$WORK/drop.log" 2>&1 &&
  [[ "$(admin -c "select count(*) from pg_database where datname = '$DB'")" == 0 ]] &&
  [[ "$(admin -c "select count(*) from pg_roles where rolname = '$ROLE'")" == 0 ]]; then
  record PASS "DROP DATABASE WITH (FORCE) with a session open, then DROP ROLE"
else
  record FAIL "DROP DATABASE WITH (FORCE) with a session open, then DROP ROLE" "$(tail -1 "$WORK/drop.log")"
fi

echo
echo "$IMAGE: $(admin -c 'show server_version')"
printf '%s\n' "${RESULTS[@]}" | grep -c '^PASS' | xargs printf 'passed: %s\n'
exit "$FAILED"
