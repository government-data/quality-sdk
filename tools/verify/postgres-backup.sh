#!/usr/bin/env bash
# Checks how pg_dump and pg_restore behave on the PostgreSQL that
# quality-data ships (decision 0003, step 8), before the agent's PostgreSQL
# adapter builds on it. Local only: one throwaway container, no account.
#
#   1. A `qs-verify-…` container of the shipped image, with two databases
#      (each with rows, an index, a sequence and a view) and a role.
#   2. `pg_dumpall --globals-only` and one `pg_dump --format=custom` per
#      database, run inside the container and streamed to files here, as
#      the adapter runs them through `docker compose exec -T`.
#   3. The dump is read back: `pg_restore --list` (its table of contents)
#      and `pg_restore --file=/dev/null` (every byte of it).
#   4. A dump cut short: whether `--list` notices, and whether a full read
#      does.
#   5. The data is changed (rows deleted, a table dropped, a table added),
#      then restored from standard input with `--clean --if-exists
#      --single-transaction`: the data is the dump's again, and the table
#      the dump did not have is still there.
#   6. A restore of a damaged dump fails and, being one transaction, leaves
#      the database as it was.
#
# The container (and its anonymous volume) is removed on exit, also on
# failure. Run alone: no build at the same time (AGENTS.md).
#
#   tools/verify/postgres-backup.sh
#
# Environment:
#   QD_POSTGRES_IMAGE   the image (default postgres:16-alpine, as the core's
#                       compose files ship it)
#
# Needs: docker.

set -euo pipefail

IMAGE="${QD_POSTGRES_IMAGE:-postgres:16-alpine}"
NAME="qs-verify-postgres-$(date -u +%Y%m%dT%H%M%SZ)-$RANDOM"
WORK="$(mktemp -d)"
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
  docker rm -f -v "$NAME" >/dev/null 2>&1
  if docker ps -a --format '{{.Names}}' | grep -qx "$NAME"; then
    echo "WARN: remove the container $NAME by hand"
    rc=1
  fi
  rm -rf "$WORK"
  exit "$rc"
}
trap cleanup EXIT

# In the container, as the superuser, as the adapter will be.
in_pg() { docker exec -i "$NAME" "$@"; }
sql() { # sql database statement: its rows, unaligned
  in_pg psql -v ON_ERROR_STOP=1 -U qd -d "$1" -Atc "$2"
}
# Everything a database holds that the check compares, as one digest.
fingerprint() {
  sql "$1" "select string_agg(t, '|' order by t) from (
      select format('%s:%s:%s', id, name, amount) as t from items
      union all select format('view:%s', count(*)) from big_items
      union all select format('seq:%s', last_value) from items_id_seq) x" | sha256sum
}

command -v docker >/dev/null || die "docker is not installed"
docker image inspect "$IMAGE" >/dev/null 2>&1 || docker pull --quiet "$IMAGE" >/dev/null

# --- 1. A database to back up ---------------------------------------------

docker run -d --name "$NAME" -e POSTGRES_USER=qd -e POSTGRES_PASSWORD=qs-verify \
  -e POSTGRES_DB=quality "$IMAGE" >/dev/null
for _ in $(seq 60); do
  in_pg pg_isready -U qd -d quality >/dev/null 2>&1 && break
  sleep 1
done
in_pg pg_isready -U qd -d quality >/dev/null || die "PostgreSQL did not start"
# The image's entrypoint restarts the server once after initdb: wait for
# a query to work twice in a row.
for _ in $(seq 30); do
  sql quality "select 1" >/dev/null 2>&1 && sleep 1 && sql quality "select 1" >/dev/null 2>&1 && break
  sleep 1
done
SERVER="$(sql quality "show server_version")"
DUMP_VERSION="$(in_pg pg_dump --version)"

sql quality "create role qs_reader login password 'qs-verify'" >/dev/null
sql quality "create database orchestrator owner qd" >/dev/null
for db in quality orchestrator; do
  sql "$db" "create table items (id serial primary key, name text not null, amount numeric);
    create index items_name on items (name);
    create view big_items as select * from items where amount > 500;
    insert into items (name, amount)
      select 'item ' || g, g * 1.5 from generate_series(1, 5000) g;
    grant select on items to qs_reader;" >/dev/null
done
BEFORE_QUALITY="$(fingerprint quality)"
BEFORE_ORCH="$(fingerprint orchestrator)"
record PASS "$IMAGE: PostgreSQL $SERVER, two databases with rows, a view, a sequence, a role"

# --- 2. Dump ------------------------------------------------------------------

if in_pg pg_dumpall -U qd --globals-only >"$WORK/globals.sql" &&
  grep -q "CREATE ROLE qs_reader" "$WORK/globals.sql"; then
  record PASS "pg_dumpall --globals-only keeps the roles"
else
  record FAIL "pg_dumpall --globals-only keeps the roles"
fi
DBS="$(sql quality "select string_agg(datname, ' ' order by datname) from pg_database
  where not datistemplate and datname <> 'postgres'")"
for db in $DBS; do
  in_pg pg_dump -U qd -d "$db" --format=custom >"$WORK/$db.dump"
done
if [[ "$DBS" == "orchestrator quality" && -s "$WORK/quality.dump" && -s "$WORK/orchestrator.dump" ]]; then
  record PASS "pg_dump --format=custom of each database, streamed out of the container" \
    "$(stat -c %s "$WORK/quality.dump") bytes"
else
  record FAIL "pg_dump of each database" "databases: $DBS"
fi

# --- 3. Read it back -----------------------------------------------------------

entries="$(in_pg pg_restore --list <"$WORK/quality.dump" | grep -cv '^;')"
if ((entries > 0)) && in_pg pg_restore --file=/dev/null <"$WORK/quality.dump"; then
  record PASS "the dump reads back: --list ($entries entries) and a full read to /dev/null"
else
  record FAIL "the dump reads back"
fi

# --- 4. A dump cut short -------------------------------------------------------

size="$(stat -c %s "$WORK/quality.dump")"
head -c $((size / 2)) "$WORK/quality.dump" >"$WORK/cut.dump"
if in_pg pg_restore --list <"$WORK/cut.dump" >/dev/null 2>&1; then
  LIST_CUT="passes"
else
  LIST_CUT="fails"
fi
if in_pg pg_restore --file=/dev/null <"$WORK/cut.dump" >/dev/null 2>"$WORK/cut.err"; then
  record FAIL "a dump cut in half fails a full read" "it was read"
else
  record PASS "a dump cut in half fails a full read" \
    "--list $LIST_CUT; full read: $(tail -1 "$WORK/cut.err")"
fi

# --- 5. Change, then restore ---------------------------------------------------

sql quality "delete from items where id % 2 = 0; drop view big_items;
  create table added_later (id int); insert into added_later values (1);
  select setval('items_id_seq', 99999);" >/dev/null
sql orchestrator "drop table items cascade" >/dev/null
for db in quality orchestrator; do
  in_pg pg_restore -U qd -d "$db" --clean --if-exists --single-transaction <"$WORK/$db.dump" \
    2>"$WORK/restore-$db.err" || record FAIL "restore $db" "$(tail -1 "$WORK/restore-$db.err")"
done
if [[ "$(fingerprint quality)" == "$BEFORE_QUALITY" && "$(fingerprint orchestrator)" == "$BEFORE_ORCH" ]]; then
  record PASS "restored from stdin with --clean --if-exists --single-transaction: rows, view and sequence are the dump's"
else
  record FAIL "restored data is the dump's"
fi
if [[ "$(sql quality "select count(*) from added_later")" == 1 ]]; then
  record PASS "a table the dump does not have is kept by --clean"
else
  record FAIL "a table the dump does not have is kept by --clean"
fi
if [[ "$(sql quality "select has_table_privilege('qs_reader', 'items', 'select')")" == t ]]; then
  record PASS "grants come back with the restore"
else
  record FAIL "grants come back with the restore"
fi

# --- 6. A damaged restore changes nothing --------------------------------------

sql quality "delete from items where id > 4000" >/dev/null
AFTER_DELETE="$(fingerprint quality)"
if in_pg pg_restore -U qd -d quality --clean --if-exists --single-transaction \
  <"$WORK/cut.dump" >/dev/null 2>"$WORK/restore-cut.err"; then
  record FAIL "restoring a cut dump fails" "it was restored"
elif [[ "$(fingerprint quality)" == "$AFTER_DELETE" ]]; then
  record PASS "restoring a cut dump fails and leaves the database as it was"
else
  record FAIL "restoring a cut dump leaves the database as it was"
fi

# --- Summary --------------------------------------------------------------

cat <<EOF

----- paste below into decision 0003, Verification -----
date: $(date -u +%Y-%m-%d)
image: $IMAGE (PostgreSQL $SERVER; $DUMP_VERSION)
$(docker version --format 'docker {{.Server.Version}}')
$(printf '%s\n' "${RESULTS[@]}")
result: $( ((FAILED)) && echo FAIL || echo PASS)
---------------------------------------------------------
EOF
exit "$FAILED"
