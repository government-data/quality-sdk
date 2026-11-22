#!/usr/bin/env bash
# Tests packaging/install.sh under --root directories, with fakes of docker,
# quality-data-agent and quality-sdk on PATH: no Docker, no AWS, no root.
# Its arguments and refusals, Docker checked and installed with
# --install-docker (fakes of apt-get, curl, gpg and dpkg), a whole install (the agent's three runs and
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
"version --format "*) echo "${QD_TEST_ENGINE:-29.10.0}" ;;
"compose version --short") echo "${QD_TEST_COMPOSE:-5.8.0}" ;;
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
# Docker's apt repository: installing docker-ce makes QD_DOCKER_BIN.
cat >"$FAKES/apt-get" <<'EOF'
#!/usr/bin/env bash
echo "apt-get $*" >>"$QD_TEST_LOG"
if [[ "$*" == *" docker-ce=5:"* ]]; then install -D -m 0755 /dev/null "$QD_DOCKER_BIN"; fi
EOF
cat >"$FAKES/curl" <<'EOF'
#!/usr/bin/env bash
echo "curl $*" >>"$QD_TEST_LOG"
while (($#)); do [[ "$1" != -o ]] || echo KEY >"$2"; shift; done
EOF
cat >"$FAKES/gpg" <<'EOF'
#!/usr/bin/env bash
case "$*" in
*--show-keys*) echo "pub:-:4096:1:8D81803C0EBFCD88:"; echo "fpr:::::::::${QD_TEST_FINGERPRINT:-9DC858229FC7DD38854AE2D88D81803C0EBFCD88}:" ;;
*--dearmor*) cat ;;
esac
EOF
printf '#!/bin/sh\necho amd64\n' >"$FAKES/dpkg"
echo helper >"$FAKES/aws_signing_helper"
chmod +x "$FAKES"/*

echo "-----BEGIN PUBLIC KEY-----" >"$WORK/index-key.pem"
echo "signed" >"$WORK/certificate.pem"
# The customer's web certificate, and a key that is not its.
for n in web other; do
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 -subj /CN=quality.example \
    -addext subjectAltName=DNS:quality.example -keyout "$WORK/$n.key" -out "$WORK/$n.crt" 2>/dev/null
done
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
refused() { # refused NAME TEXT ARGS...: exit 2, saying TEXT
  run "${ARGS[@]}" "${@:3}"
  check "$1" bash -c "test $rc = 2 && grep -q -- '$2' '$WORK/out'"
}
refused "a --tls-mode other than file or acme is refused" "file or acme" --tls-mode both
refused "--tls-mode acme without --acme-email: refused" "needs --acme-email" --tls-mode acme
refused "an --acme-email that is not an address is refused" "not an e-mail" --tls-mode acme --acme-email admin
refused "--acme-email without --tls-mode acme: refused" "acme only" --acme-email admin@example.com
refused "--tls-cert without --tls-key: refused" "go together" --tls-cert "$WORK/web.crt"
refused "--tls-cert with --tls-mode acme: refused" "file only" --tls-mode acme \
  --acme-email admin@example.com --tls-cert "$WORK/web.crt" --tls-key "$WORK/web.key"
refused "a --tls-key that is not the certificate's: refused" "not the key of" \
  --tls-cert "$WORK/web.crt" --tls-key "$WORK/other.key"
QD_SIGNING_HELPER_SHA256=0 run "${ARGS[@]}"
check "an aws_signing_helper with another SHA-256 is refused" grep -q "pinned SHA-256" "$WORK/out"
mv "$ROOT/usr/libexec/docker/cli-plugins/docker-compose" "$WORK/plugin"
run "${ARGS[@]}"
check "no system-wide Compose plugin: refused" grep -q "system-wide" "$WORK/out"
mv "$WORK/plugin" "$ROOT/usr/libexec/docker/cli-plugins/docker-compose"
QD_TEST_ENGINE=29.3.0 run "${ARGS[@]}"
check "a Docker Engine older than the floor is refused" bash -c "test $rc = 2 && grep -q 'too old' '$WORK/out'"
QD_TEST_COMPOSE=v5.0.2 run "${ARGS[@]}"
check "a Compose plugin older than the floor is refused" bash -c "test $rc = 2 && grep -q 'too old' '$WORK/out'"

# --- Docker installed with --install-docker -------------------------------------------

os_release() { # os_release ID VERSION CODENAME
  mkdir -p "$ROOT/etc"
  printf 'ID=%s\nVERSION_ID="%s"\nVERSION_CODENAME=%s\n' "$@" >"$ROOT/etc/os-release"
}
new_root no-docker
export QD_DOCKER_BIN="$ROOT/usr/bin/docker"
: >"$WORK/log"
run "${ARGS[@]}"
check "no Docker and no --install-docker: refused, naming the flag" \
  bash -c "test $rc = 2 && grep -q -- '--install-docker' '$WORK/out'"
os_release ubuntu 20.04 focal
run "${ARGS[@]}" --install-docker
check "--install-docker on Ubuntu 20.04 is refused" bash -c "test $rc = 2 && grep -q 'supports' '$WORK/out'"
os_release ubuntu 24.04 noble
QD_TEST_FINGERPRINT=0000 run "${ARGS[@]}" --install-docker
check "a Docker key with another fingerprint: nothing installed" bash -c \
  "test $rc = 1 && grep -q 'not the pinned one' '$WORK/out' && ! grep -q 'docker-ce=' '$WORK/log'"
run "${ARGS[@]}" --install-docker
check "--install-docker on Ubuntu 24.04 installs the pinned packages" grep -qx \
  "apt-get install -y -qq --no-install-recommends docker-ce=5:29.10.0-1~ubuntu.24.04~noble docker-ce-cli=5:29.10.0-1~ubuntu.24.04~noble containerd.io=2.4.2-4~ubuntu.24.04~noble docker-compose-plugin=5.8.0-1~ubuntu.24.04~noble" \
  "$WORK/log"
check "from Docker's repository, its keyring readable by apt (0644)" bash -c \
  "grep -q 'signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu noble stable' '$ROOT/etc/apt/sources.list.d/docker.list' && test $(mode "$ROOT/etc/apt/keyrings/docker.gpg" 2>/dev/null || echo 0) = 644"
check "then the install goes on (the enrolment, exit 10)" test "$rc" = 10
: >"$WORK/log"
run "${ARGS[@]}" --install-docker
check "Docker already there is not changed" bash -c \
  "grep -q 'not changed' '$WORK/out' && ! grep -q '^apt-get' '$WORK/log'"
unset QD_DOCKER_BIN
new_root root

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
check "the .env's certificate source is file, with no ACME contact" bash -c \
  "grep -qx 'QD_TLS_MODE=file' '$MOD/.env' && grep -qx 'ACME_EMAIL=' '$MOD/.env'"
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

# --- the customer's certificate, then ACME, on the installed server ---------------------

: >"$WORK/log"
run "${ARGS[@]}" --tls-cert "$WORK/web.crt" --tls-key "$WORK/web.key"
check "with --tls-cert and --tls-key it finishes (0)" test "$rc" = 0
check "the customer's certificate is Traefik's (0644), its key 0600" bash -c \
  "cmp -s '$WORK/web.crt' '$MOD/certs/local.crt' && cmp -s '$WORK/web.key' '$MOD/certs/local.key' &&
   test $(mode "$MOD/certs/local.crt") = 644 && test $(mode "$MOD/certs/local.key") = 600"
check "Traefik is recreated for it" grep -q 'up --detach --no-build --force-recreate traefik' "$WORK/log"
: >"$WORK/log"
run "${ARGS[@]}" --tls-cert "$WORK/web.crt" --tls-key "$WORK/web.key"
check "the same certificate again: Traefik is not recreated" bash -c \
  "test $rc = 0 && ! grep -q 'force-recreate' '$WORK/log'"
: >"$WORK/log"
run "${ARGS[@]}" --tls-mode acme --acme-email ops@quality.example
check "--tls-mode acme: the .env's two lines, the rest kept, Traefik recreated" bash -c \
  "test $rc = 0 && grep -qx 'QD_TLS_MODE=acme' '$MOD/.env' && grep -qx 'ACME_EMAIL=ops@quality.example' '$MOD/.env' &&
   grep -Ev '^(QD_TLS_MODE|ACME_EMAIL)=' '$MOD/.env' | cmp -s - <(grep -Ev '^(QD_TLS_MODE|ACME_EMAIL)=' '$WORK/env.before') &&
   grep -q 'force-recreate traefik' '$WORK/log'"

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
# install.json is past the stop after prepare; and an .env of the
# installer before core decision 0087 (Let's Encrypt's contact, no
# certificate source).
"$FAKES/quality-data-agent" install 2>/dev/null || true
check "the agent's start failed without PostgreSQL's certificate (step start)" \
  grep -q '"step": "start"' "$ROOT/var/lib/quality-data/updater/install.json"
sed -i -e 's/^ACME_EMAIL=.*/ACME_EMAIL=admin@example.com/' -e '/^QD_TLS_MODE=/d' "$MOD/.env"
grep -Ev '^(QD_TLS_MODE|ACME_EMAIL)=' "$MOD/.env" >"$WORK/env.before"
: >"$WORK/log"
run "${ARGS[@]}"
check "run again it finishes (0)" test "$rc" = 0
check "run again: up -d postgres and app-machine before the agent's start" \
  test "$(order)" = "$(resumed)"
check "run again the .env's certificate source is file, its ACME contact gone, the rest kept" bash -c \
  "grep -qx 'QD_TLS_MODE=file' '$MOD/.env' && grep -qx 'ACME_EMAIL=' '$MOD/.env' &&
   grep -Ev '^(QD_TLS_MODE|ACME_EMAIL)=' '$MOD/.env' | cmp -s '$WORK/env.before' -"

exit "$FAILED"
