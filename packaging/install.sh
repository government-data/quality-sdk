#!/usr/bin/env bash
# Installs Quality Data on this server (decision 0004 point 1, amended by
# decision 0006): run as root, idempotent (run again, it does only what is
# missing, and resumes an install cut halfway).
#
#   sudo packaging/install.sh --installation ID --bucket NAME \
#     --trust-anchor-arn ARN --profile-arn ARN --role-arn ARN \
#     --index-key index-key.pem --domain APP_DOMAIN --machine-name NAME \
#     --agent ./quality-data-agent --sdk ./quality-sdk \
#     [--certificate signed.pem] [--channel stable] [--region us-east-1] \
#     [--signing-helper FILE] [--version V] [--updater-name NAME] [--root DIR]
#
# 1. Docker and the system-wide Compose plugin, or it refuses.
# 2. /etc/quality-data, /opt/quality-data, /var/lib/quality-data.
# 3. The binaries in /usr/local/bin; aws_signing_helper checked by SHA-256.
# 4. The enrolment: `quality-sdk enrol` makes the key and the request; with
#    no signed certificate yet it prints the request and exits 10: have it
#    signed (`quality-sdk sign-enrolment`), then run it again with
#    --certificate.
# 5. agent.toml (no secret).
# 6. The core's files beside its versions, in /opt/quality-data/quality-data
#    (core decisions 0080, 0081, 0082): .env (0600), secrets/ (the database
#    URL for the API's user, uid 65532; PostgreSQL's password for its user,
#    uid 70), certs/ (Traefik's default certificate, self-signed), ca/,
#    internal/ and machine/.
# 7. `quality-data-agent install`, three runs (decision 0006), with the
#    core's first-start commands between them (core decision 0082): after
#    prepare (exit 4) `ca-init`, `up -d postgres` without waiting and
#    `app-machine`; after the start (exit 3) `updater-certificate` and
#    internal.yml and machines-ca.crt into internal/; the third verifies.
# 8. The agent's systemd unit, enabled.
#
# It names only quality-data, the hub every installation has.
#
# --root DIR lays everything out under DIR, never calls systemctl and, when
# not run as root, says which owner each file would get (tests).
#
# Exit status: 0 installed; 2 usage or refusal; 10 waiting for the signed
# enrolment certificate; 1 a step failed (run it again to resume).
set -euo pipefail
umask 077

HELPER_URL="https://rolesanywhere.amazonaws.com/releases/1.8.6/X86_64/Linux/Amzn2023/aws_signing_helper"
HELPER_SHA256="${QD_SIGNING_HELPER_SHA256:-3553fdc515e5682541a9ebdd37031ec7f461169aa13ccbb6fbd5b7259644113d}"
HERE="$(cd "$(dirname "$0")" && pwd)"
PG_UID=70  # PostgreSQL's image user
APP_UID=65532 # the API's and its one-shots' user

die() { echo "error: $*" >&2; exit 2; }
fail() { echo "error: $*" >&2; exit 1; }
say() { echo "install: $*"; }

declare -A A=([channel]=stable [region]=us-east-1 [updater-name]=updater)
while (($#)); do
  case "$1" in
  -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
  --*) [[ $# -ge 2 ]] || die "$1 needs a value"; A[${1#--}]="$2"; shift 2 ;;
  *) die "unexpected argument $1" ;;
  esac
done
for k in "${!A[@]}"; do
  case "$k" in
  installation | channel | region | bucket | trust-anchor-arn | profile-arn | role-arn | index-key | \
    certificate | domain | machine-name | agent | sdk | signing-helper | version | updater-name | root) ;;
  *) die "unknown option --$k" ;;
  esac
done
for k in installation bucket trust-anchor-arn profile-arn role-arn domain machine-name; do
  [[ -n "${A[$k]:-}" ]] || die "--$k is required (--help)"
done
[[ "${A[channel]}" =~ ^(stable|patch)$ ]] || die "--channel is stable or patch"
[[ "${A[installation]}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || die "--installation is not an id"

ROOT="${A[root]:-}"
if [[ -z "$ROOT" && $EUID -ne 0 ]]; then die "run it as root"; fi
ETC="$ROOT/etc/quality-data" OPT="$ROOT/opt/quality-data" VAR="$ROOT/var/lib/quality-data"
BIN="$ROOT/usr/local/bin" MOD="$OPT/quality-data"
AGENT=("$BIN/quality-data-agent") SDK=("$BIN/quality-sdk")
if [[ -n "$ROOT" ]]; then AGENT+=(--root "$ROOT"); SDK_ROOT=(--root "$ROOT"); else SDK_ROOT=(); fi

own() { # own UID PATH...
  local uid=$1; shift
  if ((EUID == 0)); then chown "$uid:$uid" "$@"; else say "owner $uid: $*"; fi
}

# --- 1. Docker and the system-wide Compose plugin -------------------------------

command -v docker >/dev/null || die "Docker is not installed"
docker compose version >/dev/null 2>&1 || die "the Docker Compose plugin is not installed"
plugin=""
for d in /usr/libexec/docker/cli-plugins /usr/lib/docker/cli-plugins /usr/local/lib/docker/cli-plugins \
  /usr/local/libexec/docker/cli-plugins; do
  [[ -x "$ROOT$d/docker-compose" ]] && plugin="$ROOT$d/docker-compose"
done
[[ -n "$plugin" ]] || die "the Compose plugin must be installed system-wide (the agent's unit hides /root/.docker)"

# --- 2. The directories ----------------------------------------------------------

mkdir -p "$ETC/enrolment" "$ETC/internal" "$MOD" "$VAR" "$BIN" "$ROOT/etc/systemd/system"
chmod 0700 "$ETC" "$ETC/enrolment" "$VAR"
# The module's folder is read by its containers' users (PostgreSQL's,
# uid 70, core#42): /opt/quality-data is only crossed.
chmod 0711 "$OPT"
chmod 0755 "$MOD"

# --- 3. The binaries ---------------------------------------------------------------

put_bin() { # put_bin SOURCE NAME
  if [[ -n "$1" ]]; then
    [[ -f "$1" ]] || die "$1 does not exist"
    install -m 0755 "$1" "$BIN/$2.new" && mv -f "$BIN/$2.new" "$BIN/$2"
  fi
  [[ -x "$BIN/$2" ]] || die "$2 is not installed: give --${3:-$2}"
}
put_bin "${A[agent]:-}" quality-data-agent agent
put_bin "${A[sdk]:-}" quality-sdk sdk
helper="${A[signing-helper]:-}"
if [[ -z "$helper" && ! -x "$BIN/aws_signing_helper" ]]; then
  helper="$(mktemp)"
  curl -fsSL --proto '=https' "$HELPER_URL" -o "$helper" || fail "could not download aws_signing_helper"
fi
if [[ -n "$helper" ]]; then
  [[ "$(sha256sum "$helper" | cut -d' ' -f1)" == "$HELPER_SHA256" ]] ||
    die "aws_signing_helper does not match its pinned SHA-256"
  install -m 0755 "$helper" "$BIN/aws_signing_helper"
fi

# --- 4. The enrolment ---------------------------------------------------------------

if [[ -n "${A[index-key]:-}" ]]; then
  [[ -f "${A[index-key]}" ]] || die "${A[index-key]} does not exist"
  install -m 0644 "${A[index-key]}" "$ETC/index-key.pem"
fi
[[ -f "$ETC/index-key.pem" ]] || die "the index's public key is not pinned: give --index-key"
if [[ ! -f "$ETC/enrolment/private-key.pem" ]]; then
  "${SDK[@]}" enrol --installation "${A[installation]}" "${SDK_ROOT[@]}" >/dev/null
fi
if [[ -n "${A[certificate]:-}" ]]; then
  [[ -f "${A[certificate]}" ]] || die "${A[certificate]} does not exist"
  install -m 0644 "${A[certificate]}" "$ETC/enrolment/certificate.pem"
fi
if [[ ! -s "$ETC/enrolment/certificate.pem" ]]; then
  echo "The enrolment request, to sign with \`quality-sdk sign-enrolment\`:"
  cat "$ETC/enrolment/request.csr"
  echo "Then run this again with --certificate <the signed certificate>."
  exit 10
fi

# --- 5. agent.toml ---------------------------------------------------------------

cat >"$ETC/agent.toml.new" <<EOF
installation = "${A[installation]}"
channel = "${A[channel]}"

[aws]
region = "${A[region]}"
bucket = "${A[bucket]}"
trust_anchor_arn = "${A[trust-anchor-arn]}"
profile_arn = "${A[profile-arn]}"
role_arn = "${A[role-arn]}"

[api]
url = "https://${A[machine-name]}:9443/api"
connect = "127.0.0.1:9443"
EOF
chmod 0644 "$ETC/agent.toml.new" && mv -f "$ETC/agent.toml.new" "$ETC/agent.toml"

# --- 6. The core's files beside its versions --------------------------------------

# The core's .env (its package's env.example, core decision 0080 point 5):
# the agent fills the IMAGE_* lines; the passwords are new.
core_env() {
  cat <<EOF
QD_MODULE_DIR=$MOD
QD_MACHINE_DIR=$MOD/machine
QD_CA_DIR=$MOD/ca
QD_UPDATER_DIR=$ETC/internal
IMAGE_API=
IMAGE_WEB=
IMAGE_TRAEFIK=
IMAGE_POSTGRES=
APP_DOMAIN=${A[domain]}
INTERNAL_DOMAIN=${A[machine-name]}
ACME_EMAIL=
OBS_VM=${A[machine-name]}
TRAEFIK_INTERNAL_PORT=9443
TRAEFIK_LAKEGATE_PORT=9444
TRAEFIK_METRICS_PORT=9445
POSTGRES_SERVER_NAME=${A[machine-name]}
POSTGRES_PORT=5432
POSTGRES_DB=knime
APP_DB_USER=knime_app
POSTGRES_SUPERUSER_PASSWORD=$(openssl rand -hex 24)
APP_DB_PASSWORD=$(openssl rand -hex 24)
EOF
}

mkdir -p "$MOD/secrets" "$MOD/certs" "$MOD/ca" "$MOD/internal" "$MOD/machine"
chmod 0755 "$MOD/secrets" "$MOD/certs" "$MOD/internal"
chmod 0700 "$MOD/ca" "$MOD/machine"
own "$APP_UID" "$MOD/ca" "$MOD/machine"
chmod 0700 "$ETC/internal"
own "$APP_UID" "$ETC/internal"
if [[ ! -f "$MOD/.env" ]]; then
  say "writing $MOD/.env and the secrets"
  core_env >"$MOD/.env.new"
  chmod 0600 "$MOD/.env.new" && mv -f "$MOD/.env.new" "$MOD/.env"
fi
env_value() { sed -n "s/^$1=//p" "$MOD/.env" | tail -1; }
if [[ ! -f "$MOD/secrets/postgres_superuser_password" ]]; then
  env_value POSTGRES_SUPERUSER_PASSWORD >"$MOD/secrets/postgres_superuser_password"
fi
if [[ ! -f "$MOD/secrets/database_url" ]]; then
  printf 'postgres://%s:%s@%s:5432/%s?sslmode=verify-full&sslrootcert=/run/qd-ca/ca.crt\n' \
    "$(env_value APP_DB_USER)" "$(env_value APP_DB_PASSWORD)" "${A[machine-name]}" \
    "$(env_value POSTGRES_DB)" >"$MOD/secrets/database_url"
fi
chmod 0400 "$MOD/secrets/postgres_superuser_password" "$MOD/secrets/database_url"
own "$PG_UID" "$MOD/secrets/postgres_superuser_password"
own "$APP_UID" "$MOD/secrets/database_url"
if [[ ! -f "$MOD/certs/local.crt" ]]; then
  # Traefik's default certificate, self-signed: no ACME without public DNS.
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 825 \
    -subj "/CN=${A[domain]}" -addext "subjectAltName=DNS:${A[domain]}" \
    -keyout "$MOD/certs/local.key" -out "$MOD/certs/local.crt" 2>/dev/null
  chmod 0644 "$MOD/certs/local.crt"
  chmod 0600 "$MOD/certs/local.key"
fi

# --- 7. The agent's install, and the core's first-start commands --------------------

pkg() { # the agent's own command line for the module
  docker compose --project-name quality-data --project-directory "$MOD/current" \
    --file "$MOD/current/compose.yml" --env-file "$MOD/.env" "$@"
}
oneshot() { # oneshot NAME ARGS...: an overlay of the package's PostgreSQL
  local name=$1; shift
  docker compose --project-name quality-data --env-file "$MOD/.env" \
    --file "$MOD/current/infra/postgres/docker-compose.yml" --file "$MOD/current/infra/postgres/tls.yml" \
    --file "$MOD/current/infra/postgres/$name.yml" --profile "$name" run --rm --no-deps "$name" "$@"
}
first_start() {
  local image
  image="$(env_value IMAGE_API)"
  [[ -n "$image" ]] || fail "$MOD/.env has no IMAGE_API after prepare"
  if [[ ! -f "$MOD/ca/ca.crt" ]]; then
    say "ca-init"
    docker run --rm --user "$APP_UID:$APP_UID" --network none -v "$MOD/ca:/run/qd-ca" \
      --entrypoint /usr/local/bin/server "$image" \
      ca-init --name "${A[installation]}" --cert /run/qd-ca/ca.crt --key /run/qd-ca/ca.key
  fi
  say "up -d postgres, without waiting"
  pkg up --detach --no-build postgres
  say "app-machine"
  local try
  for try in $(seq 30); do
    oneshot app-machine --name "${A[machine-name]}" --dir /run/qd-node && return 0
    say "PostgreSQL is not ready yet ($try)"
    sleep 2
  done
  fail "app-machine did not succeed"
}
certificates() {
  say "updater-certificate"
  oneshot updater-certificate --name "${A[updater-name]}" --dir /run/qd-updater
  local f
  for f in internal.yml machines-ca.crt; do
    local src="$MOD/current/infra/traefik/internal.yml"
    [[ "$f" == internal.yml ]] || src="$MOD/ca/ca.crt"
    # Aside then renamed into Traefik's watched folder (core decision 0081).
    install -m 0644 "$src" "$MOD/internal/.$f.new" && mv -f "$MOD/internal/.$f.new" "$MOD/internal/$f"
  done
}

if [[ -f "$VAR/updater/installed.json" && ! -f "$VAR/updater/install.json" ]]; then
  say "already installed: the agent updates it"
else
  args=(install)
  [[ -z "${A[version]:-}" ]] || args+=(--version "${A[version]}")
  for _ in 1 2 3 4; do
    rc=0
    "${AGENT[@]}" "${args[@]}" || rc=$?
    case $rc in
    0) break ;;
    4) first_start ;;
    3) certificates ;;
    *) fail "quality-data-agent install failed (exit $rc); run this again to resume" ;;
    esac
  done
  ((rc == 0)) || fail "quality-data-agent install did not finish"
fi

# --- 8. The agent's unit -------------------------------------------------------------

install -m 0644 "$HERE/systemd/quality-data-agent.service" "$ROOT/etc/systemd/system/quality-data-agent.service"
if [[ -n "$ROOT" ]]; then
  say "under --root: systemctl enable --now quality-data-agent not run"
else
  systemctl daemon-reload
  systemctl enable --now quality-data-agent
fi
say "done"
