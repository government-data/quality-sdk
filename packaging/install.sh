#!/usr/bin/env bash
# Installs Quality Data on this server (decision 0004 point 1, amended by
# decision 0006): run as root, idempotent (run again, it does only what is
# missing, and resumes an install cut halfway).
#
#   sudo packaging/install.sh --installation ID --bucket NAME \
#     --trust-anchor-arn ARN --profile-arn ARN --role-arn ARN \
#     --index-key index-key.pem --domain APP_DOMAIN --machine-name NAME \
#     --agent ./quality-data-agent --sdk ./qsdk \
#     [--certificate signed.pem] [--channel stable] [--region us-east-1] \
#     [--signing-helper FILE] [--version V] [--updater-name NAME] \
#     [--tls-mode file|acme] [--tls-cert FILE --tls-key FILE] [--acme-email ADDRESS] \
#     [--install-docker] [--admin-email ADDRESS] [--timezone ZONE] [--root DIR]
#
# 1. Docker and the system-wide Compose plugin, at least Docker Engine
#    29.4.1 and Compose 5.1.3, or it refuses. With --install-docker and no
#    Docker, on Ubuntu 24.04 or 22.04 or Debian 12, it installs Docker from
#    Docker's own apt repository (its key checked by fingerprint) at the
#    versions pinned in docker-apt.sh; a Docker already there is never
#    changed (decision 0007 point 1).
# 2. /etc/quality-data, /opt/quality-data, /var/lib/quality-data.
# 3. The binaries in /usr/local/bin; aws_signing_helper checked by SHA-256.
# 4. The enrolment: `qsdk enrol` makes the key and the request; with
#    no signed certificate yet it prints the request and exits 10: have it
#    signed (`qsdk sign-enrolment`), then run it again with
#    --certificate.
# 5. agent.toml (no secret).
# 6. The core's files beside its versions, in /opt/quality-data/quality-data
#    (core decisions 0080, 0081, 0082): .env (0600), secrets/ (the database
#    URL for the API's user, uid 65532; PostgreSQL's password for its user,
#    uid 70), certs/ (Traefik's default certificate, self-signed), ca/,
#    internal/ and machine/. The web's certificate (core decision 0087),
#    QD_TLS_MODE: `file` (the default) serves certs/local.crt, the
#    certificate of --tls-cert and --tls-key (their public keys must match)
#    or, without them, a self-signed one; `acme` is Let's Encrypt by
#    HTTP-01 (public DNS for --domain, port 80 open), and needs
#    --acme-email, its contact. A run with other values changes only those
#    lines of an existing .env; on an installed server Traefik is then
#    recreated.
# 7. `quality-data-agent install`, three runs (decision 0006), with the
#    core's first-start commands between them (core decision 0082): after
#    prepare (exit 4) `ca-init`, `up -d postgres` without waiting and
#    `app-machine`; after the start (exit 3) `updater-certificate` and
#    internal.yml and machines-ca.crt into internal/; the third verifies.
#    A run cut between them is resumed: before the agent's next run, once
#    the agent's install.json says the version is prepared, every one-shot
#    whose output is missing (ca/ca.crt, machine/machine.pem, internal/'s
#    two files once the updater's certificate is there) runs, in order.
# 8. The agent's systemd unit, enabled.
# 9. With --admin-email, once installed (this run or an earlier one): the
#    core's `server admin-access` (core decision 0086) creates that first
#    administrator, pending, and its one-time link to choose the password
#    is printed here, to this terminal only: no password exists, nothing
#    is written to a file. Run again while the account is still pending, it
#    prints a new link (decision 0007 point 2).
# 10. With --timezone, once installed: the platform's time zone (an IANA
#    name, America/Bogota or UTC) set with the core's `server timezone set`
#    (core decision 0079); the core checks the name. Without it the platform
#    stays on UTC, or on what was set before (decision 0007 point 5).
#
# It names only quality-data, the hub every installation has. A
# quality-data .env from before the core's new database names or without
# QD_TLS_MODE is refused (exit 2) before anything changes: that
# installation is moved by reinstall and restore (core decision 0088).
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
  --install-docker) A[install-docker]=yes; shift ;;
  --*) [[ $# -ge 2 ]] || die "$1 needs a value"; A[${1#--}]="$2"; shift 2 ;;
  *) die "unexpected argument $1" ;;
  esac
done
for k in "${!A[@]}"; do
  case "$k" in
  installation | channel | region | bucket | trust-anchor-arn | profile-arn | role-arn | index-key | \
    certificate | domain | machine-name | tls-mode | tls-cert | tls-key | acme-email | agent | sdk | signing-helper | \
    version | updater-name | \
    install-docker | admin-email | timezone | root) ;;
  *) die "unknown option --$k" ;;
  esac
done
for k in installation bucket trust-anchor-arn profile-arn role-arn domain machine-name; do
  [[ -n "${A[$k]:-}" ]] || die "--$k is required (--help)"
done
[[ "${A[channel]}" =~ ^(stable|patch)$ ]] || die "--channel is stable or patch"
TLS_MODE="${A[tls-mode]:-file}"
case "$TLS_MODE" in
file)
  [[ -z "${A[acme-email]:-}" ]] || die "--acme-email is for --tls-mode acme only"
  if [[ -n "${A[tls-cert]:-}${A[tls-key]:-}" ]]; then
    [[ -n "${A[tls-cert]:-}" && -n "${A[tls-key]:-}" ]] || die "--tls-cert and --tls-key go together"
  fi
  ;;
acme)
  [[ -z "${A[tls-cert]:-}${A[tls-key]:-}" ]] || die "--tls-cert and --tls-key are for --tls-mode file only"
  [[ -n "${A[acme-email]:-}" ]] || die "--tls-mode acme needs --acme-email (Let's Encrypt's contact)"
  email_re='^[^@[:space:]="\|&]+@[^@[:space:]="\|&]+\.[^@[:space:]="\|&]+$'
  [[ "${A[acme-email]}" =~ $email_re ]] || die "--acme-email is not an e-mail address (Let's Encrypt's contact)"
  ;;
*) die "--tls-mode is file or acme" ;;
esac
if [[ -n "${A[tls-cert]:-}" ]]; then
  [[ -f "${A[tls-cert]}" && -f "${A[tls-key]}" ]] || die "--tls-cert or --tls-key does not exist"
  cert_pub="$(openssl x509 -in "${A[tls-cert]}" -noout -pubkey 2>/dev/null)" ||
    die "--tls-cert is not a PEM certificate"
  # A key with a passphrase is refused too: Traefik could not read it.
  key_pub="$(openssl pkey -in "${A[tls-key]}" -passin pass: -pubout 2>/dev/null)" ||
    die "--tls-key is not a PEM private key without a passphrase"
  [[ "$cert_pub" == "$key_pub" ]] || die "--tls-key is not the key of --tls-cert"
  openssl x509 -in "${A[tls-cert]}" -noout -checkhost "${A[domain]}" 2>/dev/null | grep -q 'does match' ||
    echo "warning: --tls-cert does not name --domain ${A[domain]}" >&2
fi
[[ "${A[installation]}" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || die "--installation is not an id"
[[ -z "${A[admin-email]:-}" || "${A[admin-email]}" =~ ^[^@[:space:]=]+@[^@[:space:]=]+\.[^@[:space:]=]+$ ]] ||
  die "--admin-email is not an e-mail address"
[[ -z "${A[timezone]:-}" || "${A[timezone]}" =~ ^[A-Za-z][A-Za-z0-9_+/-]*$ ]] ||
  die "--timezone is not a time zone name (America/Bogota, UTC)"

ROOT="${A[root]:-}"
if [[ -z "$ROOT" && $EUID -ne 0 ]]; then die "run it as root"; fi
ETC="$ROOT/etc/quality-data" OPT="$ROOT/opt/quality-data" VAR="$ROOT/var/lib/quality-data"
BIN="$ROOT/usr/local/bin" MOD="$OPT/quality-data"
AGENT=("$BIN/quality-data-agent") SDK=("$BIN/qsdk")
if [[ -n "$ROOT" ]]; then AGENT+=(--root "$ROOT"); SDK_ROOT=(--root "$ROOT"); else SDK_ROOT=(); fi

# An installation made before the core's new names (quality_data,
# quality_data_app) or before QD_TLS_MODE is not installed over: it is
# moved by reinstall and restore (core decision 0088). Refused before
# anything changes.
if [[ -f "$MOD/.env" ]]; then
  old_env() { sed -n "s/^$1=//p" "$MOD/.env" | tail -1; }
  if [[ "$(old_env POSTGRES_DB)" != quality_data || "$(old_env APP_DB_USER)" != quality_data_app ]] ||
    ! grep -q '^QD_TLS_MODE=' "$MOD/.env"; then
    die "$MOD/.env predates the database names quality_data and quality_data_app or QD_TLS_MODE: \
this installation is not installed over or updated in place; move it by reinstall and restore"
  fi
fi

own() { # own UID PATH...
  local uid=$1; shift
  if ((EUID == 0)); then chown "$uid:$uid" "$@"; else say "owner $uid: $*"; fi
}

# --- 1. Docker and the system-wide Compose plugin -------------------------------

# shellcheck source=packaging/docker-apt.sh
. "$HERE/docker-apt.sh"
DOCKER_APT_ROOT="$ROOT"
SUPPORTED="Ubuntu 24.04 or 22.04, or Debian 12"
NEEDED="Docker Engine $DOCKER_FLOOR_ENGINE or newer with the Compose plugin $DOCKER_FLOOR_COMPOSE or newer"
# QD_DOCKER_BIN: the docker command looked for (the tests name an absent one).
if ! command -v "${QD_DOCKER_BIN:-docker}" >/dev/null; then
  [[ -n "${A[install-docker]:-}" ]] ||
    die "Docker is not installed: give --install-docker ($SUPPORTED), or install $NEEDED"
  distro="$(docker_apt_distro "$ROOT/etc/os-release")" ||
    die "--install-docker supports $SUPPORTED only: install $NEEDED yourself"
  read -r d_id d_version d_codename <<<"$distro"
  say "Docker $DOCKER_ENGINE_VERSION from Docker's apt repository"
  docker_apt_repository "$d_id" "$d_codename" || fail "could not add Docker's apt repository"
  read -ra packages < <(docker_apt_packages "$d_id" "$d_version" "$d_codename")
  docker_apt_install "${packages[@]}" || fail "could not install Docker"
  [[ -n "$ROOT" ]] || systemctl enable --now docker
elif [[ -n "${A[install-docker]:-}" ]]; then
  say "Docker is already installed: not changed"
fi
engine="$(docker version --format '{{.Server.Version}}' 2>/dev/null)" || die "Docker does not answer (docker version): is it running?"
docker_version_at_least "$engine" "$DOCKER_FLOOR_ENGINE" || die "Docker Engine $engine is too old: $NEEDED"
compose="$(docker compose version --short 2>/dev/null)" || die "the Docker Compose plugin is not installed"
docker_version_at_least "$compose" "$DOCKER_FLOOR_COMPOSE" || die "the Compose plugin $compose is too old: $NEEDED"
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
put_bin "${A[sdk]:-}" qsdk sdk
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
  echo "The enrolment request, to sign with \`qsdk sign-enrolment\`:"
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
QD_TLS_MODE=$TLS_MODE
ACME_EMAIL=${A[acme-email]:-}
OBS_VM=${A[machine-name]}
TRAEFIK_INTERNAL_PORT=9443
TRAEFIK_LAKEGATE_PORT=9444
TRAEFIK_METRICS_PORT=9445
POSTGRES_SERVER_NAME=${A[machine-name]}
POSTGRES_PORT=5432
POSTGRES_DB=quality_data
APP_DB_USER=quality_data_app
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
TLS_CHANGED=""
set_env() { # set_env KEY VALUE: that line of an existing .env
  [[ "$(env_value "$1")" != "$2" || "$(grep -c "^$1=" "$MOD/.env")" != 1 ]] || return 0
  say "setting $1 in $MOD/.env"
  { grep -v "^$1=" "$MOD/.env"; echo "$1=$2"; } >"$MOD/.env.new"
  chmod 0600 "$MOD/.env.new" && mv -f "$MOD/.env.new" "$MOD/.env"
  TLS_CHANGED=yes
}
set_env QD_TLS_MODE "$TLS_MODE"
set_env ACME_EMAIL "${A[acme-email]:-}"
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
if [[ -n "${A[tls-cert]:-}" ]]; then
  # The customer's certificate (checked above), aside then renamed.
  if ! cmp -s "${A[tls-cert]}" "$MOD/certs/local.crt" || ! cmp -s "${A[tls-key]}" "$MOD/certs/local.key"; then
    say "installing the certificate of --tls-cert as Traefik's"
    install -m 0600 "${A[tls-key]}" "$MOD/certs/.local.key.new"
    install -m 0644 "${A[tls-cert]}" "$MOD/certs/.local.crt.new"
    mv -f "$MOD/certs/.local.key.new" "$MOD/certs/local.key"
    mv -f "$MOD/certs/.local.crt.new" "$MOD/certs/local.crt"
    TLS_CHANGED=yes
  fi
elif [[ ! -f "$MOD/certs/local.crt" ]]; then
  # Traefik's default certificate, self-signed, until the customer's.
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
  if [[ ! -s "$MOD/ca/ca.crt" ]]; then
    say "ca-init"
    docker run --rm --user "$APP_UID:$APP_UID" --network none -v "$MOD/ca:/run/qd-ca" \
      --entrypoint /usr/local/bin/server "$image" \
      ca-init --name "${A[installation]}" --cert /run/qd-ca/ca.crt --key /run/qd-ca/ca.key
  fi
  # Done once PostgreSQL's machine certificate is there.
  [[ ! -s "$MOD/machine/machine.pem" ]] || return 0
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
  internal_files
}
internal_files() {
  local f
  for f in internal.yml machines-ca.crt; do
    local src="$MOD/current/infra/traefik/internal.yml"
    [[ "$f" == internal.yml ]] || src="$MOD/ca/ca.crt"
    # Aside then renamed into Traefik's watched folder (core decision 0081).
    install -m 0644 "$src" "$MOD/internal/.$f.new" && mv -f "$MOD/internal/.$f.new" "$MOD/internal/$f"
  done
}

# The agent's next step (install.json), empty when there is none.
install_step() { sed -n 's/.*"step" *: *"\([a-z_]*\)".*/\1/p' "$VAR/updater/install.json" 2>/dev/null | head -1; }
# A run cut between the agent's runs: the one-shots whose output is missing.
catch_up() {
  case "$(install_step)" in
  prepared | start | verify) ;;
  *) return 0 ;;
  esac
  first_start
  if [[ -s "$ETC/internal/certificate.pem" ]] &&
    ! [[ -f "$MOD/internal/internal.yml" && -f "$MOD/internal/machines-ca.crt" ]]; then
    internal_files
  fi
}

if [[ -f "$VAR/updater/installed.json" && ! -f "$VAR/updater/install.json" ]]; then
  say "already installed: the agent updates it"
  if [[ -n "$TLS_CHANGED" ]]; then
    say "recreating Traefik for the web's new certificate"
    pkg up --detach --no-build --force-recreate traefik || fail "could not recreate Traefik"
  fi
else
  catch_up
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

# --- 9. The first administrator ------------------------------------------------------

if [[ -n "${A[admin-email]:-}" ]]; then
  say "the administrator's link to choose the password (core's server admin-access):"
  pkg exec -T api /usr/local/bin/server admin-access --email "${A[admin-email]}" ||
    fail "server admin-access failed: this quality-data version may not have it"
else
  say "no --admin-email: run this again with --admin-email ADDRESS for the first administrator's link"
fi

# --- 10. The platform's time zone ---------------------------------------------------

if [[ -n "${A[timezone]:-}" ]]; then
  say "the platform's time zone (core's server timezone set):"
  pkg exec -T api /usr/local/bin/server timezone set "${A[timezone]}" ||
    fail "server timezone set ${A[timezone]} failed (see above)"
else
  say "no --timezone: the platform keeps its time zone (UTC unless set); --timezone ZONE sets it"
fi
say "done"
