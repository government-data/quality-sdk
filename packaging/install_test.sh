#!/usr/bin/env bash
# Tests packaging/install.sh under --root directories, with fakes of docker,
# quality-data-agent and quality-sdk on PATH: no Docker, no AWS, no root.
# Its arguments and refusals, a whole install (the agent's three runs and
# the core's commands between them, in core decision 0082's order), the
# files and their modes and owners, a second run doing nothing, and runs
# cut between the agent's runs then resumed.
#
#   packaging/install_test.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
FAILED=0

check() { # check NAME COMMAND...
  if "${@:2}" >/dev/null 2>&1; then echo "PASS  $1"; else echo "FAIL  $1"; FAILED=1; fi
}
mode() { stat -c %a "$1"; }

# --- the fakes --------------------------------------------------------------------

FAKES="$WORK/fakes"
mkdir -p "$FAKES"
# Docker: each one-shot writes its output; QD_TEST_FAIL fails the
# command lines that hold it.
cat >"$FAKES/docker" <<'EOF'
#!/usr/bin/env bash
echo "docker $*" >>"$QD_TEST_LOG"
[[ -z "${QD_TEST_FAIL:-}" || "$*" != *"$QD_TEST_FAIL"* ]] || exit 1
case "$*" in
*" ca-init "*) echo ca >"$QD_TEST_MOD/ca/ca.crt" ;;
*"--no-deps app-machine "*) echo pem >"$QD_TEST_MOD/machine/machine.pem" ;;
*"--no-deps updater-certificate "*) echo pem >"$QD_TEST_ROOT/etc/quality-data/internal/certificate.pem" ;;
esac
exit 0
EOF
# The agent, from its install.json as decision 0006 says: it prepares and
# exits 4; it starts (PostgreSQL is healthy only with its machine
# certificate) and exits 3 without the internal certificate; it verifies.
cat >"$FAKES/quality-data-agent" <<'EOF'
#!/usr/bin/env bash
echo "agent $*" >>"$QD_TEST_LOG"
upd="$QD_TEST_ROOT/var/lib/quality-data/updater"
mkdir -p "$upd"
at() { printf '{\n  "version": "1.0.0",\n  "step": "%s"\n}\n' "$1" >"$upd/install.json"; }
case "$(sed -n 's/.*"step": "\(.*\)".*/\1/p' "$upd/install.json" 2>/dev/null)" in
"")
  mkdir -p "$QD_TEST_MOD/1.0.0/infra/traefik"
  echo "internal" >"$QD_TEST_MOD/1.0.0/infra/traefik/internal.yml"
  ln -sfn 1.0.0 "$QD_TEST_MOD/current"
  echo "IMAGE_API=api@sha256:1" >>"$QD_TEST_MOD/.env"
  at prepared; echo '{}' >"$upd/installed.json"; exit 4 ;;
prepared | start)
  at start
  [[ -s "$QD_TEST_MOD/machine/machine.pem" ]] || { echo "postgres is unhealthy" >&2; exit 1; }
  at verify ;;
esac
[[ -s "$QD_TEST_ROOT/etc/quality-data/internal/certificate.pem" ]] || exit 3
rm -f "$upd/install.json"
EOF
# No waiting between app-machine's tries.
printf '#!/bin/sh\n' >"$FAKES/sleep"
cat >"$FAKES/quality-sdk" <<'EOF'
#!/usr/bin/env bash
echo "sdk $*" >>"$QD_TEST_LOG"
d="$QD_TEST_ROOT/etc/quality-data/enrolment"
echo key >"$d/private-key.pem"; echo "REQUEST" >"$d/request.csr"
EOF
echo helper >"$FAKES/aws_signing_helper"
chmod +x "$FAKES"/*

echo "-----BEGIN PUBLIC KEY-----" >"$WORK/index-key.pem"
echo "signed" >"$WORK/certificate.pem"
QD_SIGNING_HELPER_SHA256="$(sha256sum "$FAKES/aws_signing_helper" | cut -d' ' -f1)"
export PATH="$FAKES:$PATH" QD_TEST_LOG="$WORK/log" QD_SIGNING_HELPER_SHA256

new_root() { # new_root NAME: a server of its own, and the arguments for it
  ROOT="$WORK/$1"
  MOD="$ROOT/opt/quality-data/quality-data"
  export QD_TEST_ROOT="$ROOT" QD_TEST_MOD="$MOD"
  mkdir -p "$ROOT/usr/libexec/docker/cli-plugins"
  install -m 0755 /dev/null "$ROOT/usr/libexec/docker/cli-plugins/docker-compose"
  ARGS=(--installation inst-0001 --bucket releases --region us-east-1
    --trust-anchor-arn arn:aws:rolesanywhere:us-east-1:1:trust-anchor/a
    --profile-arn arn:aws:rolesanywhere:us-east-1:1:profile/p --role-arn arn:aws:iam::1:role/r
    --index-key "$WORK/index-key.pem" --domain quality.example --machine-name app.internal
    --acme-email admin@example.com
    --agent "$FAKES/quality-data-agent" --sdk "$FAKES/quality-sdk"
    --signing-helper "$FAKES/aws_signing_helper" --root "$ROOT")
}
new_root root
run() { rc=0; "$HERE/install.sh" "$@" >"$WORK/out" 2>&1 || rc=$?; }
# The core's commands and the agent's runs, in the order they ran.
ORDER='agent .*install|ca-init|up --detach --no-build postgres|run --rm --no-deps (app-machine|updater-certificate)'
order() { grep -oE "$ORDER" "$WORK/log" | uniq | tr '\n' ';'; }

# --- arguments and refusals ----------------------------------------------------------

run
check "no arguments: usage (2)" test "$rc" = 2
run "${ARGS[@]}" --colour blue
check "an unknown option is refused (2)" test "$rc" = 2
run "${ARGS[@]}" --channel nightly
check "a channel other than stable or patch is refused" test "$rc" = 2
run "${ARGS[@]/admin@example.com/}"
check "no --acme-email: refused (2)" bash -c "test $rc = 2 && grep -q -- '--acme-email' '$WORK/out'"
run "${ARGS[@]/admin@example.com/admin}"
check "an --acme-email that is not an address is refused" \
  bash -c "test $rc = 2 && grep -q 'not an e-mail' '$WORK/out'"
QD_SIGNING_HELPER_SHA256=0 run "${ARGS[@]}"
check "an aws_signing_helper with another SHA-256 is refused" grep -q "pinned SHA-256" "$WORK/out"
mv "$ROOT/usr/libexec/docker/cli-plugins/docker-compose" "$WORK/plugin"
run "${ARGS[@]}"
check "no system-wide Compose plugin: refused" grep -q "system-wide" "$WORK/out"
mv "$WORK/plugin" "$ROOT/usr/libexec/docker/cli-plugins/docker-compose"

# --- the enrolment waits for its certificate ------------------------------------------

run "${ARGS[@]}"
check "without a signed certificate it prints the request and exits 10" \
  bash -c "test $rc = 10 && grep -q REQUEST '$WORK/out'"
check "the agent is not run before the enrolment" bash -c "! grep -q '^agent' '$WORK/log'"

# --- a whole install --------------------------------------------------------------------

run "${ARGS[@]}" --certificate "$WORK/certificate.pem"
check "the install finishes (0)" test "$rc" = 0
check "the agent's three runs with the core's commands between them, in order" test "$(order)" = \
  "agent --root $ROOT install;ca-init;up --detach --no-build postgres;run --rm --no-deps app-machine;agent --root $ROOT install;run --rm --no-deps updater-certificate;agent --root $ROOT install;"
check "the .env has Let's Encrypt's contact" grep -qx 'ACME_EMAIL=admin@example.com' "$MOD/.env"
check "agent.toml names the installation and the internal entry point" \
  grep -qx 'url = "https://app.internal:9443/api"' "$ROOT/etc/quality-data/agent.toml"
check "the .env is 0600 and names the module's folder" \
  bash -c "test $(mode "$MOD/.env") = 600 && grep -qx 'QD_MODULE_DIR=$MOD' '$MOD/.env'"
pw="$(sed -n 's/^APP_DB_PASSWORD=//p' "$MOD/.env")"
check "the database URL carries the .env's password, verify-full by the machine's name" \
  grep -q "knime_app:$pw@app.internal:5432/knime?sslmode=verify-full" "$MOD/secrets/database_url"
check "PostgreSQL's password file is its user's (uid 70)" \
  grep -q "owner 70: $MOD/secrets/postgres_superuser_password" "$WORK/out"
check "the database URL is the API's user's (uid 65532)" \
  grep -q "owner 65532: $MOD/secrets/database_url" "$WORK/out"
check "the secrets are 0400 in a folder the containers can read (0755)" bash -c \
  "test $(mode "$MOD/secrets/database_url") = 400 && test $(mode "$MOD/secrets") = 755 && test $(mode "$MOD") = 755"
check "/opt/quality-data is only crossed (0711); /etc/quality-data is 0700" bash -c \
  "test $(mode "$ROOT/opt/quality-data") = 711 && test $(mode "$ROOT/etc/quality-data") = 700"
check "internal.yml and machines-ca.crt are in Traefik's watched folder" \
  test -f "$MOD/internal/internal.yml" -a -f "$MOD/internal/machines-ca.crt"
check "a self-signed default certificate for Traefik" test -s "$MOD/certs/local.crt"
check "the unit is installed, not enabled under --root" bash -c \
  "test -f '$ROOT/etc/systemd/system/quality-data-agent.service' && grep -q 'not run' '$WORK/out'"

# --- run again: nothing more ---------------------------------------------------------

cp "$MOD/.env" "$WORK/env.before"
: >"$WORK/log"
run "${ARGS[@]}"
check "run again it finishes (0)" test "$rc" = 0
check "run again it neither installs nor runs a command" \
  bash -c "! grep -Eq '^agent|^sdk|ca-init|up --detach|run --rm' '$WORK/log'"
check "run again the .env and its passwords are kept" cmp -s "$WORK/env.before" "$MOD/.env"

# --- cut after ca-init (PostgreSQL does not start), run again -----------------------

resumed() { # what a resumed run does: PostgreSQL's one-shots, then the agent's start
  echo "up --detach --no-build postgres;run --rm --no-deps app-machine;agent --root $ROOT install;run --rm --no-deps updater-certificate;agent --root $ROOT install;"
}
new_root cut-after-the-ca
: >"$WORK/log"
QD_TEST_FAIL="up --detach" run "${ARGS[@]}" --certificate "$WORK/certificate.pem"
check "a failed up -d postgres stops the install (1)" test "$rc" = 1
check "it stopped after ca-init, before app-machine" test "$(order)" = \
  "agent --root $ROOT install;ca-init;up --detach --no-build postgres;"
: >"$WORK/log"
run "${ARGS[@]}"
check "run again it finishes (0)" test "$rc" = 0
check "run again: up -d postgres and app-machine before the agent's start, not ca-init" \
  test "$(order)" = "$(resumed)"
check "internal.yml and machines-ca.crt are in place" \
  test -f "$MOD/internal/internal.yml" -a -f "$MOD/internal/machines-ca.crt"

# --- cut at app-machine, the agent's start tried, run again ----------------------------

new_root cut-at-the-machine
: >"$WORK/log"
QD_TEST_FAIL="app-machine" run "${ARGS[@]}" --certificate "$WORK/certificate.pem"
check "a failing app-machine stops the install (1)" \
  bash -c "test $rc = 1 && grep -q 'app-machine did not succeed' '$WORK/out'"
# As on the first test installation: the agent's start was tried, so its
# install.json is past the stop after prepare; and an .env written without
# Let's Encrypt's contact.
"$FAKES/quality-data-agent" install 2>/dev/null || true
check "the agent's start failed without PostgreSQL's certificate (step start)" \
  grep -q '"step": "start"' "$ROOT/var/lib/quality-data/updater/install.json"
sed -i 's/^ACME_EMAIL=.*/ACME_EMAIL=/' "$MOD/.env"
grep -v '^ACME_EMAIL=' "$MOD/.env" >"$WORK/env.before"
: >"$WORK/log"
run "${ARGS[@]}"
check "run again it finishes (0)" test "$rc" = 0
check "run again: up -d postgres and app-machine before the agent's start" \
  test "$(order)" = "$(resumed)"
check "run again the empty ACME_EMAIL is set, the rest of the .env kept" bash -c \
  "grep -qx 'ACME_EMAIL=admin@example.com' '$MOD/.env' && grep -v '^ACME_EMAIL=' '$MOD/.env' | cmp -s '$WORK/env.before' -"

exit "$FAILED"
