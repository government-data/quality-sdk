#!/usr/bin/env bash
# The docs module added to and removed from an installation end to end, on
# the dev path (decision 0002 step 9 and the amendment to steps 3, 7 and 9).
# Local only: no account, no AWS.
#
# What is real: the agent's `qd module add|remove` logic (agent::modules)
# with its Docker Compose adapter, package files and route files, run by
# the example `module_offline` (the signed index replaced by a local
# package and image); the docs module's module.yaml and compose.yml and its
# image (the docs repository's CI artifact `image-docs`); Traefik v3.6 with
# the core package's folder layout and the core's middlewares
# (infra/traefik/dynamic of core's main). What stands in: the core's
# /auth/verify gate, a small server answering as it does (its behaviour is
# tested against PostgreSQL in core's crates/api/tests/verify_it.rs), and
# quality-data's PUT /updater/v1/modules, printed instead of sent (core's
# crates/api/tests/updater_v1_it.rs takes the same body).
#
# It shows:
# The docs module is main's of the docs repository; its image, the CI
# artifact of the pull request main's commit came from.
#
#   1. before: /docs/ is not served (404);
#   2. `add docs`: the docs container runs healthy, its route file
#      module-docs.yml is in Traefik's watched folder, /docs/ with a session
#      is the guide (200, the guide's own CSP), a page without one goes to
#      sign in and back, and the modules told carry docs' route and menu
#      entry in English and Spanish;
#   3. `remove quality-data` is refused (the core);
#   4. `remove docs`: its container is gone (its project's volumes kept),
#      its route file is gone, /docs/ is 404 again, and the modules told no
#      longer have docs.
#
# Needs: docker, gh (signed in, to read the docs and core repositories and
# download the image artifact), tar, curl, and the example built first, one
# build at a time (AGENTS.md):
#
#   capped cargo build -j 2 -p agent --example module_offline
#
# Docker names: the containers qs-verify-mdocs-*, and what the docs
# module's compose.yml names itself: the project `docs` and the network
# `data-quality-edge`; it refuses to start if either exists. All of it is
# removed on exit, also on failure. Run alone: no build at the same time.
#
#   tools/verify/module-docs.sh
#
# Environment:
#   QS_VERIFY_PORT   Traefik's port on 127.0.0.1 (default 18443)
#   CARGO_TARGET_DIR where the example was built (default ./target)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
PORT="${QS_VERIFY_PORT:-18443}"
BIN="${CARGO_TARGET_DIR:-$ROOT_DIR/target}/debug/examples/module_offline"
NAME=qs-verify-mdocs
NET=data-quality-edge
WORK="$(mktemp -d)"
FAILED=0

pass() { echo "PASS  $*"; }
fail() { echo "FAIL  $*"; FAILED=1; }

cleanup() {
  docker rm -f "$NAME-traefik" "$NAME-api" >/dev/null 2>&1 || true
  docker ps -aq --filter label=com.docker.compose.project=docs | xargs -r docker rm -f >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}

[ -x "$BIN" ] || { echo "build the example first: $BIN"; exit 2; }
if docker network inspect "$NET" >/dev/null 2>&1 ||
  [ -n "$(docker ps -aq --filter label=com.docker.compose.project=docs)" ]; then
  echo "the network $NET or a project docs exists: not touching them"; exit 2
fi
trap cleanup EXIT INT TERM

# The docs module at its main: manifest, compose, and its CI image.
# CI runs on pull requests: the image is that of the pull request main's
# commit was squashed from (the same files).
SHA="$(gh api repos/government-data/docs/commits/main --jq .sha)"
HEAD="$(gh api "repos/government-data/docs/commits/$SHA/pulls" --jq '.[0].head.sha // empty')"
RUN="$(gh run list -R government-data/docs --commit "${HEAD:-$SHA}" --workflow ci.yml \
  --status success --json databaseId --jq '.[0].databaseId // empty')"
[ -n "$RUN" ] || { echo "no successful CI run of docs for $SHA"; exit 2; }
gh run download "$RUN" -R government-data/docs -n image-docs -D "$WORK/image" >/dev/null
IMAGE="$(docker load -i "$WORK/image/qd-docs.tar" | sed -n 's/^Loaded image: //p' | tail -1)"
mkdir -p "$WORK/pkg"
for f in module.yaml compose.yml; do
  gh api "repos/government-data/docs/contents/$f?ref=$SHA" --jq .content | base64 -d >"$WORK/pkg/$f"
done
VERSION="$(sed -n 's/^version: //p' "$WORK/pkg/module.yaml")"
PACKAGE="$WORK/$VERSION.tar.gz"
tar -C "$WORK/pkg" -czf "$PACKAGE" module.yaml compose.yml
mkdir -p "$WORK/core"
gh api "repos/government-data/core/contents/infra/traefik/dynamic/middlewares.yml" --jq .content |
  base64 -d >"$WORK/core/middlewares.yml"

# The installation: quality-data installed at 1.4.0, Traefik's watched
# folder (its `internal/`) and the package's dynamic files inside it.
R="$WORK/root"
INTERNAL="$R/opt/quality-data/quality-data/internal"
mkdir -p "$INTERNAL" "$R/var/lib/quality-data/updater" "$WORK/certs"
cat >"$R/var/lib/quality-data/updater/installed.json" <<'EOF'
{"version": "1.4.0", "modules": {"quality-data": {"version": "1.4.0", "images": {}}},
 "order": ["quality-data"]}
EOF
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 -subj "/CN=localhost" \
  -keyout "$WORK/certs/local.key" -out "$WORK/certs/local.crt" >/dev/null 2>&1
chmod 0644 "$WORK/certs/local.key"
cat >"$WORK/traefik.yml" <<'EOF'
global: { checkNewVersion: false, sendAnonymousUsage: false }
entryPoints:
  websecure: { address: ":443" }
  traefik: { address: ":8080" }
ping: {}
providers:
  file: { directory: /etc/traefik/dynamic, watch: true }
EOF
cat >"$WORK/gate.py" <<'EOF'
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import quote
class Gate(BaseHTTPRequestHandler):
    def do_GET(self):
        if "qd_session=ok" in (self.headers.get("Cookie") or ""):
            self.send_response(204)
            for h in ["X-Auth-User", "X-Auth-Email", "X-Auth-Role", "X-Auth-Org"]:
                self.send_header(h, "x")
        elif "text/html" in (self.headers.get("Accept") or ""):
            self.send_response(302)
            origin = "https://" + (self.headers.get("X-Forwarded-Host") or "")
            uri = self.headers.get("X-Forwarded-Uri") or "/"
            self.send_header("Location", origin + "/login?next=" + quote(uri, safe="/") + "&page=1")
        else:
            self.send_response(401)
        self.end_headers()
    def log_message(self, *a): pass
HTTPServer(("0.0.0.0", 8080), Gate).serve_forever()
EOF

docker network create "$NET" >/dev/null
docker run -d --name "$NAME-api" --network "$NET" --network-alias api \
  -v "$WORK/gate.py:/gate.py:ro" python:3.12-slim python /gate.py >/dev/null
docker run -d --name "$NAME-traefik" --network "$NET" -p "127.0.0.1:$PORT:443" \
  -e APP_DOMAIN=localhost -e QD_TLS_MODE=file \
  -v "$WORK/traefik.yml:/etc/traefik/traefik.yml:ro" \
  -v "$INTERNAL:/etc/traefik/dynamic" \
  -v "$WORK/core:/etc/traefik/dynamic/package:ro" \
  -v "$WORK/certs:/etc/traefik/certs:ro" \
  traefik:v3.6 --configFile=/etc/traefik/traefik.yml >/dev/null
for _ in $(seq 1 30); do
  docker exec "$NAME-traefik" traefik healthcheck --ping >/dev/null 2>&1 && break
  sleep 1
done

URL="https://localhost:$PORT"
code() { curl -sk -o /dev/null -w '%{http_code}' "$@"; }
until_code() {
  local path="$1" want="$2" got=""
  shift 2
  for _ in $(seq 1 20); do
    got="$(code "$@" "$URL$path")"
    [ "$got" = "$want" ] && return 0
    sleep 1
  done
  echo "  last answer: $got" >&2
  return 1
}
HTML=(-H "Accept: text/html")
SESSION=(-H "Cookie: qd_session=ok")

if until_code /docs/ 404 "${HTML[@]}" "${SESSION[@]}"; then pass "before: /docs/ is 404"; else fail "before: /docs/"; fi

if TOLD="$("$BIN" --root "$R" add docs "$PACKAGE" "web=$IMAGE")"; then
  pass "add docs"
else
  fail "add docs"
fi
state="$(docker ps --filter label=com.docker.compose.project=docs --format '{{.Status}}')"
[[ "$state" == *healthy* ]] && pass "docs runs: $state" || fail "docs container: ${state:-none}"
[ -f "$INTERNAL/module-docs.yml" ] && pass "route file module-docs.yml written" || fail "no route file"
if until_code /docs/ 200 "${HTML[@]}" "${SESSION[@]}"; then pass "/docs/ with a session: 200"; else fail "/docs/ with a session"; fi
csp="$(curl -sk -D - -o /dev/null "${SESSION[@]}" "$URL/docs/" | grep -i '^content-security-policy:' || true)"
[[ "$csp" == *wasm-unsafe-eval* ]] && pass "the guide's own CSP" || fail "the guide's CSP: ${csp:-none}"
loc="$(curl -sk -o /dev/null -w '%{redirect_url}' "${HTML[@]}" "$URL/docs/es/")"
[ "$loc" = "https://localhost:$PORT/login?next=/docs/es/&page=1" ] &&
  pass "without a session: to sign in and back" || fail "without a session: $loc"
if grep -q '"route_path":"/docs"' <<<"$TOLD" && grep -q '"es":"/docs/es/"' <<<"$TOLD" &&
  grep -q 'Guía de usuario' <<<"$TOLD"; then
  pass "told: docs with its route and menu entry"
else
  fail "told: $TOLD"
fi

if "$BIN" --root "$R" remove quality-data 2>"$WORK/err"; then
  fail "remove quality-data should be refused"
else
  grep -q "the core" "$WORK/err" && pass "remove quality-data refused" || fail "refusal: $(cat "$WORK/err")"
fi

if TOLD="$("$BIN" --root "$R" remove docs)"; then pass "remove docs"; else fail "remove docs"; fi
[ -z "$(docker ps -aq --filter label=com.docker.compose.project=docs)" ] &&
  pass "the docs container is gone" || fail "docs containers remain"
[ ! -f "$INTERNAL/module-docs.yml" ] && pass "route file removed" || fail "route file remains"
if until_code /docs/ 404 "${HTML[@]}" "${SESSION[@]}"; then pass "after: /docs/ is 404"; else fail "after: /docs/"; fi
grep -q '"docs"' <<<"$TOLD" && fail "told after remove: $TOLD" || pass "told: docs no longer"
[ -f "$R/opt/quality-data/docs/.env" ] && pass "docs' .env and package kept" || fail "docs' files gone"

if [ "$FAILED" = 0 ]; then echo "Result: PASS"; else echo "Result: FAIL"; exit 1; fi
