# shellcheck shell=bash
# The core as a module's OIDC and purge checks need it (decision 0009,
# Verification V3 and V4), sourced by module-oidc.sh and module-purge.sh.
# Local only: no account, no AWS, invented values.
#
# What is real: the core's API (QD_API_IMAGE, the CI artifact `image-api`
# of core's main, never built here) with its migrations, its OIDC provider,
# its CA (`server ca-init`) and the update agent's machine certificate
# (`server updater-certificate`); PostgreSQL 16 as the core ships it, as the
# Compose project `quality-data` at /opt/quality-data/quality-data/current
# under the run's root, where the agent's PostgreSQL adapter runs its
# programs (`docker compose exec -T postgres`); Traefik v3.6 with the core's
# middlewares.yml and internal.yml (core's main), on its `internal` entry
# point with the agent's certificate required, and on `websecure` for
# people, the agent's route files watched. What is left out: the core's web
# and node agent, TLS to PostgreSQL (V2 verified it), the real ports.
#
# Ports, on 127.0.0.1 only: QS_VERIFY_PORT (websecure, default 18443) and
# QS_VERIFY_INTERNAL_PORT (internal, default 19443). The names: containers
# qs-verify-core-*, the Compose project `quality-data`, and the fixed network
# data-quality-edge the modules join; it refuses to start if any exists.
#
# Sets: R (the installation's root), CORE (quality-data's folder), WEB (the
# web origin), API_URL (the agent's `updater/v1` base), ADMIN_EMAIL and
# ADMIN_PASSWORD (an invented account), and core_* functions. The caller
# sets WORK, traps core_down, and calls core_up.

PORT="${QS_VERIFY_PORT:-18443}"
IPORT="${QS_VERIFY_INTERNAL_PORT:-19443}"
DOMAIN=qd.qs-verify.test
INTERNAL_DOMAIN=qs-verify-internal.test
CN=qs-verify-core
EDGE=data-quality-edge
WEB="https://$DOMAIN:$PORT"
API_URL="https://$INTERNAL_DOMAIN:$IPORT/api"
ADMIN_EMAIL=admin@qs-verify.test
ADMIN_PASSWORD="Qs-Verify-$RANDOM$RANDOM-Pw1"
ALPINE="${ALPINE_IMAGE:-alpine:3}"
SDK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# The core's files at main, read through gh (no checkout needed).
core_file() { # PATH DEST
  gh api "repos/government-data/core/contents/$1?ref=main" --jq .content | base64 -d >"$2"
}

core_compose() { # ARGS: the core stand-in's project, as the agent runs it
  docker compose --project-name quality-data --project-directory "$CORE/current" \
    --file "$CORE/current/compose.yml" --env-file "$CORE/.env" "$@"
}

as_root() { # SCRIPT: as root in a throwaway container with the work folder
  docker run --rm --network none -v "$WORK:$WORK" "$ALPINE" sh -c "$1"
}

core_down() {
  if [ -f "$CORE/current/compose.yml" ]; then
    core_compose down --volumes --remove-orphans --timeout 2 >/dev/null 2>&1
  fi
  docker rm -f "$CN-traefik" "$CN-api" >/dev/null 2>&1
  docker network rm "$EDGE" "$CN-net" >/dev/null 2>&1
  as_root "rm -rf -- '${WORK:?}/root' '${WORK:?}/ca' '${WORK:?}/updater' '${WORK:?}/secrets'" >/dev/null 2>&1
}

core_preflight() {
  : "${QD_API_IMAGE:?set QD_API_IMAGE to the API image of the CI artifact image-api of core main}"
  docker image inspect "$QD_API_IMAGE" >/dev/null 2>&1 || { echo "load $QD_API_IMAGE first"; return 1; }
  for n in "$EDGE" "$CN-net"; do
    if docker network inspect "$n" >/dev/null 2>&1; then echo "the network $n exists"; return 1; fi
  done
  if [ -n "$(docker ps -aq --filter label=com.docker.compose.project=quality-data)" ]; then
    echo "a Compose project quality-data exists"; return 1
  fi
  for p in "$PORT" "$IPORT"; do
    if ss -Hltn "sport = :$p" | grep -q .; then echo "port $p is in use"; return 1; fi
  done
}

# The CA, the API's keys, PostgreSQL and the API, the agent's certificate,
# Traefik. Fails (returns non-zero) when the stand-in does not answer.
core_up() {
  R="$WORK/root"
  CORE="$R/opt/quality-data/quality-data"
  mkdir -p "$CORE/current" "$CORE/internal" "$WORK/ca" "$WORK/updater" "$WORK/traefik" \
    "$R/var/lib/quality-data/updater" "$R/etc/quality-data/internal"
  chmod 755 "$WORK"
  cat >"$R/var/lib/quality-data/updater/installed.json" <<'EOF'
{"version": "1.4.0", "modules": {"quality-data": {"version": "1.4.0", "images": {}}},
 "order": ["quality-data"]}
EOF
  local app_pw super_pw settings
  app_pw="$(openssl rand -hex 16)"
  super_pw="$(openssl rand -hex 16)"
  settings="$(openssl rand -base64 32)"
  # PostgreSQL as the core's: its first-init role scripts (the app's, the
  # queue's by certificate, the metrics').
  mkdir -p "$WORK/initdb"
  for f in 10-app-role.sh 20-queue-role.sh 30-metrics-role.sh; do
    core_file "infra/postgres/initdb/$f" "$WORK/initdb/$f"
  done
  chmod 0755 "$WORK/initdb" "$WORK"/initdb/*
  cat >"$CORE/current/compose.yml" <<EOF
services:
  postgres:
    image: postgres:16-alpine
    container_name: $CN-postgres
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: \${SUPER_PW}
      POSTGRES_DB: quality_data
      APP_DB_USER: quality_data_app
      APP_DB_PASSWORD: \${APP_PW}
      QD_QUEUE_AUTH: certificate
    volumes:
      - $WORK/initdb:/docker-entrypoint-initdb.d:ro
    networks: [core]
    healthcheck:
      test: ["CMD", "pg_isready", "-U", "postgres", "-d", "quality_data"]
      interval: 2s
      retries: 30
networks:
  core:
    name: $CN-net
EOF
  printf 'SUPER_PW=%s\nAPP_PW=%s\nPOSTGRES_SERVER_NAME=%s-postgres\n' "$super_pw" "$app_pw" "$CN" >"$CORE/.env"
  core_compose up --detach --wait --wait-timeout 90 postgres >"$WORK/core-up.log" 2>&1 ||
    { echo "PostgreSQL: $(tail -2 "$WORK/core-up.log")"; return 1; }
  local db="postgres://quality_data_app:$app_pw@$CN-postgres:5432/quality_data?sslmode=disable"

  # The CA, as the installer makes it (the image's user owns its folder).
  as_root "chown 65532:65532 '$WORK/ca' '$WORK/updater'"
  docker run --rm --user 65532:65532 --network none -v "$WORK/ca:/run/qd-ca" \
    --entrypoint /usr/local/bin/server "$QD_API_IMAGE" ca-init --name "qs verify" \
    --cert /run/qd-ca/ca.crt --key /run/qd-ca/ca.key >"$WORK/ca.log" 2>&1 ||
    { echo "ca-init: $(tail -2 "$WORK/ca.log")"; return 1; }
  # The OIDC provider's signing keys as the installer makes them (its
  # core_secret, decision 0012 step 4), read by the API from files as the
  # core's package passes them: no key set by hand.
  # shellcheck source=/dev/null
  source <(sed -n '/^core_secret() {/,/^}/p' "$SDK_ROOT/packaging/install.sh")
  mkdir -p "$WORK/secrets"
  core_secret ed25519 >"$WORK/secrets/oidc-signing-key"
  core_secret p256 >"$WORK/secrets/oidc-signing-key-es256"
  as_root "chown 65532:65532 '$WORK/secrets'/* && chmod 0400 '$WORK/secrets'/*"
  local apienv=(-e APP_HOST=0.0.0.0 -e APP_PORT=8080 -e "DATABASE_URL=$db"
    -e "APP_PUBLIC_URL=$WEB" -e "OAUTH_REDIRECT_BASE=$WEB/api" -e "INTERNAL_DOMAIN=$INTERNAL_DOMAIN"
    -e "TRAEFIK_INTERNAL_PORT=$IPORT" -e INTERNAL_CLIENT_CERT_HEADER=trusted
    -e CA_CERT_FILE=/run/qd-ca/ca.crt -e CA_KEY_FILE=/run/qd-ca/ca.key
    -e "SETTINGS_ENCRYPTION_KEY=$settings" -e OIDC_SIGNING_KEY_FILE=/run/secrets/oidc-signing-key
    -e OIDC_SIGNING_KEY_ES256_FILE=/run/secrets/oidc-signing-key-es256 -v "$WORK/secrets:/run/secrets:ro"
    -e "ADMIN_BOOTSTRAP_EMAIL=$ADMIN_EMAIL" -e "ADMIN_BOOTSTRAP_PASSWORD=$ADMIN_PASSWORD"
    -e OBS_VM=qs-verify -v "$WORK/ca:/run/qd-ca:ro")
  docker run -d --name "$CN-api" --network "$CN-net" --network-alias api "${apienv[@]}" \
    "$QD_API_IMAGE" >/dev/null
  local up=""
  for _ in $(seq 60); do
    docker logs "$CN-api" 2>&1 | grep -q '"msg":"listening"' && { up=yes; break; }
    sleep 1
  done
  [ -n "$up" ] || { echo "the API did not start: $(docker logs --tail 5 "$CN-api" 2>&1)"; return 1; }
  if docker logs "$CN-api" 2>&1 | grep -q 'OIDC provider is not configured'; then
    echo "the API did not take the installer's OIDC signing keys"; return 1
  fi
  docker run --rm --network "$CN-net" "${apienv[@]}" --entrypoint /usr/local/bin/server \
    "$QD_API_IMAGE" bootstrap-admin >"$WORK/admin.log" 2>&1 ||
    { echo "bootstrap-admin: $(tail -2 "$WORK/admin.log")"; return 1; }
  # The update agent's machine and its files, into the agent's folder.
  docker run --rm --network "$CN-net" -e "DATABASE_URL=$db" -e CA_CERT_FILE=/run/qd-ca/ca.crt \
    -e CA_KEY_FILE=/run/qd-ca/ca.key -v "$WORK/ca:/run/qd-ca:ro" -v "$WORK/updater:/run/qd-updater" \
    --entrypoint /usr/local/bin/server "$QD_API_IMAGE" updater-certificate --name qs-verify-updater \
    --dir /run/qd-updater >"$WORK/updater.log" 2>&1 ||
    { echo "updater-certificate: $(tail -2 "$WORK/updater.log")"; return 1; }
  as_root "cp '$WORK/updater/ca.pem' '$WORK/updater/certificate.pem' '$WORK/updater/private-key.pem' \
    '$R/etc/quality-data/internal/' && cp '$WORK/ca/ca.crt' '$WORK/traefik/machines-ca.crt' &&
    chown -R $(id -u) '$R/etc/quality-data/internal' '$WORK/traefik/machines-ca.crt'"
  # The installation's CA, which the agent copies for a module's database.
  cp "$R/etc/quality-data/internal/ca.pem" "$R/etc/quality-data/internal/ca.pem.bak" 2>/dev/null || true

  # Traefik: the internal entry point's certificate from the CA's key.
  as_root "cat '$WORK/ca/ca.key'" >"$WORK/traefik/ca.key"
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj "/CN=$INTERNAL_DOMAIN" \
    -keyout "$WORK/traefik/internal.key" -out "$WORK/traefik/internal.csr" 2>/dev/null
  printf 'subjectAltName=DNS:%s\n' "$INTERNAL_DOMAIN" >"$WORK/traefik/san"
  openssl x509 -req -in "$WORK/traefik/internal.csr" -CA "$WORK/traefik/machines-ca.crt" \
    -CAkey "$WORK/traefik/ca.key" -CAcreateserial -days 1 -extfile "$WORK/traefik/san" \
    -out "$WORK/traefik/internal.crt" 2>/dev/null
  rm -f "$WORK/traefik/ca.key"
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
    -subj "/CN=$DOMAIN" -addext "subjectAltName=DNS:$DOMAIN" \
    -keyout "$WORK/traefik/web.key" -out "$WORK/traefik/web.crt" 2>/dev/null
  chmod 0644 "$WORK"/traefik/*
  core_file infra/traefik/dynamic/middlewares.yml "$WORK/traefik/middlewares.yml"
  core_file infra/traefik/internal.yml "$CORE/internal/internal.yml"
  cp "$WORK/traefik/machines-ca.crt" "$WORK/traefik/internal.crt" "$WORK/traefik/internal.key" "$CORE/internal/"
  cat >"$CORE/internal/core.yml" <<EOF
tls:
  certificates:
    - { certFile: /etc/traefik/web/web.crt, keyFile: /etc/traefik/web/web.key }
http:
  routers:
    api:
      rule: "Host(\`$DOMAIN\`) && PathPrefix(\`/api\`) && !PathPrefix(\`/api/updater/\`)"
      entryPoints: [websecure]
      tls: {}
      service: api-svc
      middlewares: [api-strip@file]
  services:
    api-svc:
      loadBalancer:
        servers: [{ url: "http://api:8080" }]
EOF
  cat >"$WORK/traefik/traefik.yml" <<EOF
global: { checkNewVersion: false, sendAnonymousUsage: false }
entryPoints:
  websecure: { address: ":$PORT" }
  internal: { address: ":$IPORT" }
  traefik: { address: ":8080" }
ping: {}
providers:
  file: { directory: /etc/traefik/dynamic, watch: true }
EOF
  docker network create "$EDGE" >/dev/null
  docker create --name "$CN-traefik" --network "$CN-net" --network-alias "$INTERNAL_DOMAIN" \
    -p "127.0.0.1:$PORT:$PORT" -p "127.0.0.1:$IPORT:$IPORT" \
    -e "APP_DOMAIN=$DOMAIN" -e QD_TLS_MODE=file -e "INTERNAL_DOMAIN=$INTERNAL_DOMAIN" \
    -e INTERNAL_API_SERVICE=api-svc@file \
    -v "$WORK/traefik/traefik.yml:/etc/traefik/traefik.yml:ro" \
    -v "$CORE/internal:/etc/traefik/dynamic" \
    -v "$WORK/traefik/middlewares.yml:/etc/traefik/dynamic/package/middlewares.yml:ro" \
    -v "$WORK/traefik:/etc/traefik/web:ro" \
    traefik:v3.6 --configFile=/etc/traefik/traefik.yml >/dev/null
  docker network connect --alias "$DOMAIN" "$EDGE" "$CN-traefik"
  docker start "$CN-traefik" >/dev/null
  for _ in $(seq 30); do
    [ "$(core_agent_call GET /updater/v1/version -o /dev/null -w '%{http_code}')" = 200 ] && return 0
    sleep 1
  done
  echo "the internal entry point does not answer: $(docker logs --tail 5 "$CN-traefik" 2>&1)"
  return 1
}

core_agent_call() { # METHOD PATH [curl args]: as the agent, with its certificate
  local method=$1 path=$2
  shift 2
  local d="$R/etc/quality-data/internal"
  curl -s -X "$method" --max-time 10 --cacert "$d/ca.pem" --cert "$d/certificate.pem" \
    --key "$d/private-key.pem" --resolve "$INTERNAL_DOMAIN:$IPORT:127.0.0.1" "$@" "$API_URL$path"
}

core_web() { # PATH [curl args]: a person's request on the web entry point
  local path=$1
  shift
  curl -s --max-time 10 --cacert "$WORK/traefik/web.crt" --resolve "$DOMAIN:$PORT:127.0.0.1" "$@" "$WEB$path"
}

core_sql() { # SQL: as PostgreSQL's superuser, on the core's database
  docker exec "$CN-postgres" psql -U postgres -d quality_data -XAtc "$1"
}

# The agent's settings for module_offline: the internal entry point by name,
# reached on the loopback.
core_agent_args() {
  printf '%s\n' --api "$API_URL" --connect "127.0.0.1:$IPORT"
}
