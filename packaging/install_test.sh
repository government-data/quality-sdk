#!/usr/bin/env bash
# Tests packaging/install.sh under --root directories, with fakes of docker,
# quality-data-agent and quality-sdk on PATH: no Docker, no AWS, no root.
# Its arguments and refusals, a whole install (the agent's three runs and
# the core's commands between them, in core decision 0082's order), the
# files and their modes and owners, and a second run doing nothing.
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
cat >"$FAKES/docker" <<'EOF'
#!/usr/bin/env bash
echo "docker $*" >>"$QD_TEST_LOG"
[[ "$*" == *" ca-init "* ]] && : >"$QD_TEST_MOD/ca/ca.crt"
exit 0
EOF
# The agent: install exits 4, then 3, then 0, as decision 0006 says, and
# lays out a version when it prepares.
cat >"$FAKES/quality-data-agent" <<'EOF'
#!/usr/bin/env bash
echo "agent $*" >>"$QD_TEST_LOG"
n=$(($(cat "$QD_TEST_RUNS" 2>/dev/null || echo 0) + 1))
echo "$n" >"$QD_TEST_RUNS"
upd="$QD_TEST_ROOT/var/lib/quality-data/updater"
mkdir -p "$upd"
case $n in
1)
  mkdir -p "$QD_TEST_MOD/1.0.0/infra/traefik"
  echo "internal" >"$QD_TEST_MOD/1.0.0/infra/traefik/internal.yml"
  ln -sfn 1.0.0 "$QD_TEST_MOD/current"
  echo "IMAGE_API=api@sha256:1" >>"$QD_TEST_MOD/.env"
  echo '{}' >"$upd/install.json"; echo '{}' >"$upd/installed.json"; exit 4 ;;
2) exit 3 ;;
*) rm -f "$upd/install.json"; exit 0 ;;
esac
EOF
cat >"$FAKES/quality-sdk" <<'EOF'
#!/usr/bin/env bash
echo "sdk $*" >>"$QD_TEST_LOG"
d="$QD_TEST_ROOT/etc/quality-data/enrolment"
echo key >"$d/private-key.pem"; echo "REQUEST" >"$d/request.csr"
EOF
echo helper >"$FAKES/aws_signing_helper"
chmod +x "$FAKES"/*

ROOT="$WORK/root"
MOD="$ROOT/opt/quality-data/quality-data"
mkdir -p "$ROOT/usr/libexec/docker/cli-plugins"
install -m 0755 /dev/null "$ROOT/usr/libexec/docker/cli-plugins/docker-compose"
echo "-----BEGIN PUBLIC KEY-----" >"$WORK/index-key.pem"
echo "signed" >"$WORK/certificate.pem"
QD_SIGNING_HELPER_SHA256="$(sha256sum "$FAKES/aws_signing_helper" | cut -d' ' -f1)"
export PATH="$FAKES:$PATH" QD_TEST_ROOT="$ROOT" QD_TEST_MOD="$MOD" \
  QD_TEST_LOG="$WORK/log" QD_TEST_RUNS="$WORK/runs" QD_SIGNING_HELPER_SHA256

ARGS=(--installation inst-0001 --bucket releases --region us-east-1
  --trust-anchor-arn arn:aws:rolesanywhere:us-east-1:1:trust-anchor/a
  --profile-arn arn:aws:rolesanywhere:us-east-1:1:profile/p --role-arn arn:aws:iam::1:role/r
  --index-key "$WORK/index-key.pem" --domain quality.example --machine-name app.internal
  --agent "$FAKES/quality-data-agent" --sdk "$FAKES/quality-sdk"
  --signing-helper "$FAKES/aws_signing_helper" --root "$ROOT")
run() { rc=0; "$HERE/install.sh" "$@" >"$WORK/out" 2>&1 || rc=$?; }

# --- arguments and refusals ----------------------------------------------------------

run
check "no arguments: usage (2)" test "$rc" = 2
run "${ARGS[@]}" --colour blue
check "an unknown option is refused (2)" test "$rc" = 2
run "${ARGS[@]}" --channel nightly
check "a channel other than stable or patch is refused" test "$rc" = 2
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
order="$(grep -oE 'agent .*install|ca-init|up --detach --no-build postgres|run --rm --no-deps (app-machine|updater-certificate)' "$WORK/log" | tr '\n' ';')"
check "the agent's three runs with the core's commands between them, in order" test "$order" = \
  "agent --root $ROOT install;ca-init;up --detach --no-build postgres;run --rm --no-deps app-machine;agent --root $ROOT install;run --rm --no-deps updater-certificate;agent --root $ROOT install;"
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

exit "$FAILED"
