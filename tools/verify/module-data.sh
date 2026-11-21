#!/usr/bin/env bash
# Checks how Docker Compose behaves where the agent reads a module
# manifest's `health` and backs up its `data` (decisions 0002 and 0003),
# before the agent builds on it. Local only: one throwaway project, no
# account.
#
# A module laid out as the agent lays it out (`<module>/current/compose.yml`,
# `<module>/.env`), with one service `app` that mounts a named volume at
# /data, run as the adapter runs Compose (`--project-name`,
# `--project-directory`, `--file`, `--env-file`):
#
#   1. `exec -T app <program> <args>` gives the program's exit status: 0,
#      and 3 for a program that exits 3; `exec` of a service with no running
#      container fails.
#   2. `timeout --kill-after=5 <secs> docker compose exec -T …` of a command
#      that hangs ends with 124 within the time.
#   3. `cp app:/data -` writes a tar archive of the volume to standard
#      output (entries under `data/`), from a running container and from a
#      stopped one; it ends with the end-of-archive marker (two zero
#      blocks), which an archive cut short lacks (`tar -t` alone may read a
#      cut archive without an error).
#   4. `cp - app:/` reads that archive from standard input into a stopped
#      container: the files the archive has are its own again, and a file
#      it does not have is kept.
#   5. A stopped container's volume is emptied by a throwaway container of
#      its own image with its mounts (`docker run --rm --volumes-from <id>
#      --user 0:0 --network none --entrypoint find <image> <path> -mindepth
#      1 -delete`): its named volume and its anonymous one are the
#      container's own (`docker inspect` lists both mounts), files of
#      another owner go too, and the mount point stays. Then `cp - app:/`
#      leaves the archive's files only: the restore the agent does.
#
# The project's containers, network and volume are removed on exit, also on
# failure. Run alone: no build at the same time (AGENTS.md).
#
#   tools/verify/module-data.sh
#
# Environment:
#   QD_IMAGE   a local image to run, by tag (default alpine:3); it must have a
#              registry digest (pulled, not built here)
#
# Needs: docker with the compose plugin, tar, coreutils `timeout`.

set -euo pipefail

TAG="${QD_IMAGE:-alpine:3}"
PROJECT="qs-verify-data-$(date -u +%Y%m%dt%H%M%Sz)-$RANDOM"
WORK="$(mktemp -d)"
MODULE="$WORK/opt/quality-data/qs-verify"
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

compose() {
  docker compose --project-name "$PROJECT" --project-directory "$MODULE/current" \
    --file "$MODULE/current/compose.yml" --env-file "$MODULE/.env" "$@"
}

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local rc=$?
  set +e
  if [[ -e "$MODULE/current" ]]; then
    compose down -v --remove-orphans --timeout 1 >/dev/null 2>&1
  fi
  docker rm -f "$PROJECT-empty" >/dev/null 2>&1
  docker ps -aq --filter "label=com.docker.compose.project=$PROJECT" | xargs -r docker rm -f -v >/dev/null
  docker volume ls -q --filter "label=com.docker.compose.project=$PROJECT" | xargs -r docker volume rm >/dev/null
  docker network ls -q --filter "label=com.docker.compose.project=$PROJECT" | xargs -r docker network rm >/dev/null
  if [[ -n "$(docker ps -aq --filter "label=com.docker.compose.project=$PROJECT")$(docker volume ls -q --filter "label=com.docker.compose.project=$PROJECT")" ]]; then
    echo "WARN: remove what is left of the project $PROJECT by hand"
    rc=1
  fi
  rm -rf "$WORK"
  exit "$rc"
}
trap cleanup EXIT

docker compose version >/dev/null || die "docker compose is not installed"
command -v timeout >/dev/null || die "timeout (coreutils) is not installed"
IMAGE="$(docker image inspect "$TAG" --format '{{index .RepoDigests 0}}' 2>/dev/null)" ||
  die "$TAG is not a local image with a registry digest (docker pull $TAG)"

mkdir -p "$MODULE/1.0.0"
cat >"$MODULE/1.0.0/compose.yml" <<'EOF'
services:
  app:
    image: ${IMAGE_APP:?IMAGE_APP is the agent's}
    command: ["sleep", "3600"]
    stop_grace_period: 1s
    volumes: ["data:/data", "/scratch"]
  idle:
    image: ${IMAGE_APP}
    command: ["true"]
    restart: "no"
volumes:
  data: {}
EOF
printf 'IMAGE_APP=%s\n' "$IMAGE" >"$MODULE/.env"
ln -s 1.0.0 "$MODULE/current"
compose up -d --no-build app >"$WORK/up.log" 2>&1 || die "up: $(tail -1 "$WORK/up.log")"

# --- 1. exec gives the exit status --------------------------------------------

compose exec -T app true && ok=0 || ok=$?
compose exec -T app sh -c 'exit 3' && three=0 || three=$?
if [[ "$ok" == 0 && "$three" == 3 ]]; then
  record PASS "exec -T gives the program's exit status" "0 and 3"
else
  record FAIL "exec -T gives the program's exit status" "got $ok and $three"
fi
if compose exec -T idle true >"$WORK/idle.log" 2>&1; then
  record FAIL "exec of a service without a running container fails" "it returned 0"
else
  record PASS "exec of a service without a running container fails" "$(tail -1 "$WORK/idle.log")"
fi

# --- 2. timeout ends a check that hangs ---------------------------------------

start=$SECONDS
timeout --kill-after=5 3 docker compose --project-name "$PROJECT" \
  --project-directory "$MODULE/current" --file "$MODULE/current/compose.yml" \
  --env-file "$MODULE/.env" exec -T app sleep 60 && hung=0 || hung=$?
took=$((SECONDS - start))
if [[ "$hung" == 124 && "$took" -le 10 ]]; then
  record PASS "timeout ends exec of a command that hangs with 124" "after ${took}s"
else
  record FAIL "timeout ends exec of a command that hangs" "status $hung after ${took}s"
fi

# --- 3. cp of the volume to standard output -----------------------------------

compose exec -T app sh -c 'mkdir -p /data/sub && echo one > /data/a && echo two > /data/sub/b'
compose cp app:/data - >"$WORK/running.tar" 2>"$WORK/cp.log" || true
if tar -tf "$WORK/running.tar" >"$WORK/list" 2>/dev/null &&
  grep -qx 'data/a' "$WORK/list" && grep -qx 'data/sub/b' "$WORK/list"; then
  record PASS "cp app:/data - writes a tar of the volume, entries under data/" \
    "$(wc -l <"$WORK/list") entries, from a running container"
else
  record FAIL "cp app:/data - writes a tar of the volume" "$(tail -1 "$WORK/cp.log")"
fi
compose stop app >/dev/null 2>&1
if compose cp app:/data - >"$WORK/stopped.tar" 2>"$WORK/cp2.log" &&
  tar -tf "$WORK/stopped.tar" | grep -qx 'data/sub/b'; then
  record PASS "cp app:/data - reads the volume of a stopped container"
else
  record FAIL "cp app:/data - reads the volume of a stopped container" "$(tail -1 "$WORK/cp2.log")"
fi
# The end of an archive: whole 512-byte blocks, the last two all zero.
ends_whole() {
  local size
  size="$(stat -c %s "$1")"
  ((size >= 1024 && size % 512 == 0)) &&
    [[ -z "$(tail -c 1024 "$1" | tr -d '\0' | head -c 1)" ]]
}
if ends_whole "$WORK/running.tar" && ends_whole "$WORK/stopped.tar"; then
  record PASS "the archive ends with the end-of-archive marker (two zero blocks)" \
    "$(stat -c %s "$WORK/running.tar") bytes"
else
  record FAIL "the archive ends with the end-of-archive marker"
fi
head -c 2000 "$WORK/running.tar" >"$WORK/cut.tar"
listed=PASS
tar -tf "$WORK/cut.tar" >/dev/null 2>&1 || listed=FAIL
if ! ends_whole "$WORK/cut.tar"; then
  record PASS "an archive cut short lacks the end-of-archive marker" \
    "tar -t alone: $( [[ $listed == PASS ]] && echo 'reads it without an error' || echo 'refuses it')"
else
  record FAIL "an archive cut short lacks the end-of-archive marker"
fi

# --- 4. cp of the archive back into a stopped container -----------------------

compose start app >/dev/null 2>&1
compose exec -T app sh -c 'echo changed > /data/a && rm /data/sub/b && echo new > /data/c'
compose stop app >/dev/null 2>&1
if compose cp - app:/ <"$WORK/running.tar" 2>"$WORK/cp3.log"; then
  compose start app >/dev/null 2>&1
  got="$(compose exec -T app sh -c 'cat /data/a /data/sub/b /data/c' 2>/dev/null | tr '\n' ' ')"
  if [[ "$got" == "one two new " ]]; then
    record PASS "cp - app:/ restores the archive's files into a stopped container" \
      "a file it does not have is kept"
  else
    record FAIL "cp - app:/ restores the archive's files" "read: $got"
  fi
else
  record FAIL "cp - app:/ into a stopped container" "$(tail -1 "$WORK/cp3.log")"
fi

# --- 5. empty the volume of a stopped container, then the archive ------------

compose start app >/dev/null 2>&1
compose exec -T app sh -c 'echo later > /data/c && mkdir -p /data/d && echo x > /data/d/e &&
  chown 1234:1234 /data/c /data/d/e && echo s > /scratch/s'
compose stop app >/dev/null 2>&1
cid="$(compose ps --all --quiet app)"
image="$(docker inspect --format '{{.Image}}' "$cid")"
mounts="$(docker inspect --format '{{range .Mounts}}{{.Destination}} {{end}}' "$cid")"
empty() { # empty <path>: the agent's command
  docker run --rm --name "$PROJECT-empty" --network none --user 0:0 --volumes-from "$cid" \
    --entrypoint find "$image" "$1" -mindepth 1 -delete
}
left() { # left <path>: what the stopped container's volume holds
  compose cp "app:$1" - 2>/dev/null | tar -t 2>/dev/null | tr '\n' ' '
}
if [[ " $mounts" == *" /data "* && " $mounts" == *" /scratch "* ]] &&
  empty /data 2>"$WORK/empty.log" && empty /scratch 2>>"$WORK/empty.log"; then
  data_left="$(left /data)"
  scratch_left="$(left /scratch)"
  if [[ "$data_left" == "data/ " && "$scratch_left" == "scratch/ " ]]; then
    record PASS "docker run --volumes-from empties a stopped container's named and anonymous volumes" \
      "files of uid 1234 too; the mount points stay"
  else
    record FAIL "docker run --volumes-from empties the volumes" "left: $data_left| $scratch_left"
  fi
else
  record FAIL "docker run --volumes-from empties the volumes" \
    "mounts: $mounts; $(tail -1 "$WORK/empty.log" 2>/dev/null)"
fi
if compose cp - app:/ <"$WORK/running.tar" 2>"$WORK/cp4.log"; then
  compose start app >/dev/null 2>&1
  got="$(compose exec -T app sh -c 'cd /data && find . | sort | tr "\n" " "; cat a sub/b' 2>/dev/null | tr '\n' ' ')"
  if [[ "$got" == ". ./a ./sub ./sub/b one two " ]]; then
    record PASS "emptied, then cp - app:/: the volume holds the archive's files only"
  else
    record FAIL "emptied, then cp - app:/" "read: $got"
  fi
else
  record FAIL "cp - app:/ into the emptied volume" "$(tail -1 "$WORK/cp4.log")"
fi

# --- Summary --------------------------------------------------------------

cat <<EOF

----- paste below into decision 0003, Verification -----
date: $(date -u +%Y-%m-%d)
$(docker version --format 'docker {{.Server.Version}}'); $(docker compose version); $(timeout --version | head -1)
image: $IMAGE
$(printf '%s\n' "${RESULTS[@]}")
result: $( ((FAILED)) && echo FAIL || echo PASS)
---------------------------------------------------------
EOF
exit "$FAILED"
