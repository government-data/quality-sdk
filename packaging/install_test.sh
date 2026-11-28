#!/usr/bin/env bash
# Tests packaging/install.sh under --root directories, with fakes of docker,
# quality-data-agent on PATH: no Docker, no AWS, no root.
# Its arguments and refusals, Docker checked and installed with
# --install-docker (fakes of apt-get, curl, gpg and dpkg), a whole install (the agent's three runs and
# the core's commands between them, in core decision 0082's order), the
# files and their modes and owners, a second run doing nothing, runs
# cut between the agent's runs then resumed, qd linked to the agent (another
# qd refused), the agent enrolling (--sdk refused), and an .env from before the
# core's new names or QD_TLS_MODE refused.
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
*" exec -T api /usr/local/bin/server timezone set Not/AZone") echo "timezone: not a time zone" >&2; exit 1 ;;
*" exec -T api /usr/local/bin/server timezone set "*) echo "${*##* }" ;;
*" exec -T api /usr/local/bin/server admin-access "*) printf 'choose the password at\nhttps://quality.example/activate#TOKEN\n' ;;
esac
exit 0
EOF
# The agent, from its install.json as decision 0006 says: it prepares and
# exits 4; it starts (PostgreSQL is healthy only with its machine
# certificate) and exits 3 without the internal certificate; it verifies.
cat >"$FAKES/quality-data-agent" <<'EOF'
#!/usr/bin/env bash
echo "agent $*" >>"$QD_TEST_LOG"
if [[ "$*" == *" enrol --installation "* ]]; then
  d="$QD_TEST_ROOT/etc/quality-data/enrolment"
  echo key >"$d/private-key.pem"; echo "REQUEST" >"$d/request.csr"
  exit 0
fi
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
    --agent "$FAKES/quality-data-agent"
    --signing-helper "$FAKES/aws_signing_helper" --root "$ROOT")
}
new_root root
run() { rc=0; "$HERE/install.sh" "$@" >"$WORK/out" 2>&1 || rc=$?; }
# The core's commands and the agent's runs, in the order they ran.
ORDER='agent( --root [^ ]+)? install|ca-init|up --detach --no-build postgres|run --rm --no-deps (app-machine|updater-certificate)'
order() { grep -oE "$ORDER" "$WORK/log" | uniq | tr '\n' ';'; }

# --- arguments and refusals ----------------------------------------------------------

run
check "no arguments: usage (2)" test "$rc" = 2
run "${ARGS[@]}" --colour blue
check "an unknown option is refused (2)" test "$rc" = 2
run "${ARGS[@]}" --channel nightly
check "a channel other than stable or patch is refused" test "$rc" = 2
run "${ARGS[@]}" --address app.example
check "an --address that is not an IP address is refused" bash -c "test $rc = 2 && grep -q -- '--address' '$WORK/out'"
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
new_root other-qd
mkdir -p "$ROOT/usr/local/bin" && echo mine >"$ROOT/usr/local/bin/qd"
run "${ARGS[@]}"
check "a qd that is not the agent's link is refused (2), kept, nothing laid out" bash -c \
  "test $rc = 2 && grep -q 'not the link to quality-data-agent' '$WORK/out' && grep -qx mine '$ROOT/usr/local/bin/qd' && ! test -e '$ROOT/etc/quality-data'"
rm "$ROOT/usr/local/bin/qd" && ln -s /usr/bin/true "$ROOT/usr/local/bin/qd"
run "${ARGS[@]}"
check "a qd linked elsewhere is refused (2) and kept" bash -c \
  "test $rc = 2 && test \"\$(readlink '$ROOT/usr/local/bin/qd')\" = /usr/bin/true"
new_root with-sdk
run "${ARGS[@]}" --sdk "$WORK/qsdk"
check "--sdk is refused (2), saying the agent enrols, nothing laid out" bash -c \
  "test $rc = 2 && grep -q 'qd enrol' '$WORK/out' && ! test -e '$ROOT/etc/quality-data'"
new_root old-sdk
mkdir -p "$ROOT/usr/local/bin" && echo old >"$ROOT/usr/local/bin/qsdk"
run "${ARGS[@]}"
check "a qsdk left by an earlier install is noticed and kept" bash -c \
  "test $rc = 10 && grep -q 'qsdk is no longer used' '$WORK/out' && grep -qx old '$ROOT/usr/local/bin/qsdk'"
new_root root

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
check "the agent enrols the server, with no qsdk" bash -c \
  "grep -qx 'agent --root $ROOT enrol --installation inst-0001' '$WORK/log' && ! test -e '$ROOT/usr/local/bin/qsdk'"
check "the agent's install is not run before the enrolment" bash -c "! grep -Eq '^agent( --root [^ ]+)? install' '$WORK/log'"

# --- a whole install --------------------------------------------------------------------

run "${ARGS[@]}" --certificate "$WORK/certificate.pem" --address 10.0.0.5
check "the install finishes (0)" test "$rc" = 0
check "the agent's three runs with the core's commands between them, in order" test "$(order)" = \
  "agent --root $ROOT install;ca-init;up --detach --no-build postgres;run --rm --no-deps app-machine;agent --root $ROOT install;run --rm --no-deps updater-certificate;agent --root $ROOT install;"
check "the .env's certificate source is file, with no ACME contact" bash -c \
  "grep -qx 'QD_TLS_MODE=file' '$MOD/.env' && grep -qx 'ACME_EMAIL=' '$MOD/.env'"
check "the .env carries the server's --address" grep -qx 'QD_CORE_ADDRESS=10.0.0.5' "$MOD/.env"
check "agent.toml names the installation and the internal entry point" \
  grep -qx 'url = "https://app.internal/api"' "$ROOT/etc/quality-data/agent.toml"
check "the .env is 0600 and names the module's folder" \
  bash -c "test $(mode "$MOD/.env") = 600 && grep -qx 'QD_MODULE_DIR=$MOD' '$MOD/.env'"
pw="$(sed -n 's/^APP_DB_PASSWORD=//p' "$MOD/.env")"
check "the database URL carries the .env's password, verify-full by the machine's name" \
  grep -q "quality_data_app:$pw@app.internal:5432/quality_data?sslmode=verify-full" "$MOD/secrets/database_url"
check "PostgreSQL's password file is its user's (uid 70)" \
  grep -q "owner 70: $MOD/secrets/postgres_superuser_password" "$WORK/out"
check "the database URL is the API's user's (uid 65532)" \
  grep -q "owner 65532: $MOD/secrets/database_url" "$WORK/out"
check "Dolt's two passwords are the API's user's (uid 65532), 43 base64url characters" bash -c \
  "grep -q 'owner 65532: .*/dolt-root-password$' '$WORK/out' &&
   grep -q 'owner 65532: .*/dolt-app-password$' '$WORK/out' &&
   grep -Eqx '[A-Za-z0-9_-]{43}' '$MOD/secrets/dolt-root-password' &&
   grep -Eqx '[A-Za-z0-9_-]{43}' '$MOD/secrets/dolt-app-password' &&
   test $(mode "$MOD/secrets/dolt-app-password") = 400 &&
   ! cmp -s '$MOD/secrets/dolt-root-password' '$MOD/secrets/dolt-app-password'"
check "the lake's STS issuer's keys: 16 and 32 bytes base64url, 0400, the API's user's" bash -c \
  "grep -Eqx '[A-Za-z0-9_-]{22}' '$MOD/secrets/rustfs-sts-issuer-access-key' &&
   grep -Eqx '[A-Za-z0-9_-]{43}' '$MOD/secrets/rustfs-sts-issuer-secret-key' &&
   test $(mode "$MOD/secrets/rustfs-sts-issuer-access-key") = 400 &&
   test $(mode "$MOD/secrets/rustfs-sts-issuer-secret-key") = 400 &&
   grep -q 'owner 65532: .*/rustfs-sts-issuer-access-key$' '$WORK/out' &&
   grep -q 'owner 65532: .*/rustfs-sts-issuer-secret-key$' '$WORK/out'"
check "the OIDC signing keys: PKCS#8 in base64, Ed25519 and P-256, no newline, 0400" bash -c \
  "base64 -d '$MOD/secrets/oidc-signing-key' | openssl pkey -inform DER -noout -text | grep -q ED25519 &&
   base64 -d '$MOD/secrets/oidc-signing-key-es256' | openssl pkey -inform DER -noout -text | grep -q prime256v1 &&
   base64 -d '$MOD/secrets/oidc-signing-key' | openssl pkcs8 -nocrypt -inform DER >/dev/null &&
   base64 -d '$MOD/secrets/oidc-signing-key-es256' | openssl pkcs8 -nocrypt -inform DER >/dev/null &&
   test \$(wc -l <'$MOD/secrets/oidc-signing-key') = 0 &&
   test \$(wc -l <'$MOD/secrets/oidc-signing-key-es256') = 0 &&
   test $(mode "$MOD/secrets/oidc-signing-key") = 400 &&
   grep -q 'owner 65532: .*/oidc-signing-key-es256$' '$WORK/out'"
check "the secrets are 0400 in a folder the containers can read (0755)" bash -c \
  "test $(mode "$MOD/secrets/database_url") = 400 && test $(mode "$MOD/secrets") = 755 && test $(mode "$MOD") = 755"
check "/opt/quality-data is only crossed (0711); /etc/quality-data is 0700" bash -c \
  "test $(mode "$ROOT/opt/quality-data") = 711 && test $(mode "$ROOT/etc/quality-data") = 700"
check "internal.yml and machines-ca.crt are in Traefik's watched folder" \
  test -f "$MOD/internal/internal.yml" -a -f "$MOD/internal/machines-ca.crt"
check "a self-signed default certificate for Traefik" test -s "$MOD/certs/local.crt"
check "the unit is installed, not enabled under --root" bash -c \
  "test -f '$ROOT/etc/systemd/system/quality-data-agent.service' && grep -q 'not run' '$WORK/out'"
check "qd is a link to quality-data-agent beside it" \
  test "$(readlink "$ROOT/usr/local/bin/qd")" = quality-data-agent

# --- run again: nothing more ---------------------------------------------------------

cp "$MOD/.env" "$WORK/env.before"
cp "$MOD/secrets/dolt-app-password" "$WORK/dolt.before"
cp "$MOD/secrets/oidc-signing-key" "$WORK/oidc.before"
: >"$WORK/log"
run "${ARGS[@]}"
check "run again it finishes (0)" test "$rc" = 0
check "run again it neither installs nor runs a command" \
  bash -c "! grep -Eq '^agent|ca-init|up --detach|run --rm' '$WORK/log'"
check "run again the .env and its passwords are kept" cmp -s "$WORK/env.before" "$MOD/.env"
check "run again Dolt's password is kept" cmp -s "$WORK/dolt.before" "$MOD/secrets/dolt-app-password"
check "run again the OIDC signing key is kept" cmp -s "$WORK/oidc.before" "$MOD/secrets/oidc-signing-key"
check "run again qd is still the link to quality-data-agent" \
  test "$(readlink "$ROOT/usr/local/bin/qd")" = quality-data-agent
check "without --admin-email it says how to get the administrator's link" grep -q -- '--admin-email' "$WORK/out"

# --- the first administrator -----------------------------------------------------------

run "${ARGS[@]}" --admin-email admin
check "an --admin-email that is not an address is refused (2)" test "$rc" = 2
: >"$WORK/log"
run "${ARGS[@]}" --admin-email admin@example.com
check "--admin-email asks the core's server admin-access, once installed" grep -qx \
  "docker compose --project-name quality-data --project-directory $MOD/current --file $MOD/current/compose.yml --env-file $MOD/.env exec -T api /usr/local/bin/server admin-access --email admin@example.com" \
  "$WORK/log"
check "the link is printed to the terminal and written to no file" bash -c \
  "test $rc = 0 && grep -q 'activate#TOKEN' '$WORK/out' && ! grep -rq 'TOKEN' '$ROOT'"
QD_TEST_FAIL="admin-access" run "${ARGS[@]}" --admin-email admin@example.com
check "a failing server admin-access fails the run (1)" test "$rc" = 1

# --- the platform's time zone ----------------------------------------------------------

run "${ARGS[@]}" --timezone 'Bogota; rm'
check "a --timezone that is not a zone name is refused (2)" test "$rc" = 2
: >"$WORK/log"
run "${ARGS[@]}" --timezone America/Bogota
check "--timezone sets it with the core's server timezone set, once installed" bash -c \
  "test $rc = 0 && grep -q 'exec -T api /usr/local/bin/server timezone set America/Bogota\$' '$WORK/log'"
run "${ARGS[@]}" --timezone Not/AZone
check "a zone the core refuses fails the run (1), with the core's reason" \
  bash -c "test $rc = 1 && grep -q 'not a time zone' '$WORK/out'"

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
# certificate source), refused by core decision 0088.
"$FAKES/quality-data-agent" install 2>/dev/null || true
check "the agent's start failed without PostgreSQL's certificate (step start)" \
  grep -q '"step": "start"' "$ROOT/var/lib/quality-data/updater/install.json"
sed -i -e 's/^ACME_EMAIL=.*/ACME_EMAIL=admin@example.com/' -e '/^QD_TLS_MODE=/d' "$MOD/.env"
cp "$MOD/.env" "$WORK/env.old"
: >"$WORK/log"
run "${ARGS[@]}"
check "an .env without QD_TLS_MODE is refused (2), saying reinstall and restore" \
  bash -c "test $rc = 2 && grep -q 'reinstall and restore' '$WORK/out'"
check "refused: the .env byte for byte as it was, and nothing run" \
  bash -c "cmp -s '$WORK/env.old' '$MOD/.env' && ! test -s '$WORK/log'"
# The person states the source; the run resumes and the .env follows it.
echo "QD_TLS_MODE=acme" >>"$MOD/.env"
grep -Ev '^(QD_TLS_MODE|ACME_EMAIL)=' "$MOD/.env" >"$WORK/env.before"
: >"$WORK/log"
run "${ARGS[@]}"
check "run again it finishes (0)" test "$rc" = 0
check "run again: up -d postgres and app-machine before the agent's start" \
  test "$(order)" = "$(resumed)"
check "run again the .env's certificate source is file, its ACME contact gone, the rest kept" bash -c \
  "grep -qx 'QD_TLS_MODE=file' '$MOD/.env' && grep -qx 'ACME_EMAIL=' '$MOD/.env' &&
   grep -Ev '^(QD_TLS_MODE|ACME_EMAIL)=' '$MOD/.env' | cmp -s '$WORK/env.before' -"

# --- an .env with the legacy database names (core decision 0088) -----------------------

for legacy in 's/^POSTGRES_DB=.*/POSTGRES_DB=knime/' 's/^APP_DB_USER=.*/APP_DB_USER=knime_app/'; do
  new_root "legacy-${legacy:3:6}"
  run "${ARGS[@]}" --certificate "$WORK/certificate.pem"
  sed -i "$legacy" "$MOD/.env"
  cp "$MOD/.env" "$WORK/env.old"
  : >"$WORK/log"
  run "${ARGS[@]}" --certificate "$WORK/certificate.pem"
  check "an .env with a legacy name ($legacy) is refused (2), nothing changed or run" bash -c \
    "test $rc = 2 && grep -q 'quality_data_app' '$WORK/out' && cmp -s '$WORK/env.old' '$MOD/.env' &&
     ! test -s '$WORK/log'"
done

exit "$FAILED"
