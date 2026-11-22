#!/usr/bin/env bash
# Checks the installer's --install-docker against Docker's real apt
# repository (decision 0007 point 1), before the installer uses it. Local
# only: throwaway containers, no account.
#
# For every distribution packaging/docker-apt.sh supports, in a container of
# its official image, with that file's own functions:
#
#   1. /etc/os-release is recognised as supported.
#   2. Docker's signing key, downloaded, has the pinned fingerprint, and the
#      repository is added with it.
#   3. The pinned packages resolve: `apt-get install --dry-run` of the
#      package=version list succeeds, and says which versions it would set
#      up.
#
# And an unsupported distribution (ubuntu:20.04) is refused by step 1.
#
# The containers (qs-verify-docker-…) are removed on exit, also on failure.
# Run alone: no build at the same time (AGENTS.md).
#
#   tools/verify/docker-apt.sh
#
# Needs: docker, and HTTPS to the distributions' mirrors and
# download.docker.com.

set -euo pipefail

LIB="$(cd "$(dirname "$0")/../../packaging" && pwd)/docker-apt.sh"
RUN="qs-verify-docker-$(date -u +%Y%m%dt%H%M%Sz)-$RANDOM"
FAILED=0
RESULTS=()
NAMES=()

record() { # record PASS|FAIL name [detail]
  local line="$1  $2${3:+ ($3)}"
  echo "$line"
  RESULTS+=("$line")
  if [[ "$1" == FAIL ]]; then FAILED=1; fi
}

# shellcheck disable=SC2329 # invoked by the EXIT trap below
cleanup() {
  local name
  for name in "${NAMES[@]}"; do docker rm -f "$name" >/dev/null 2>&1 || true; done
}
trap cleanup EXIT

# The checks inside a container: the library mounted read-only.
INSIDE="$(
  cat <<'EOF'
set -euo pipefail
. /qs/docker-apt.sh
read -r id version codename < <(docker_apt_distro /etc/os-release) || { echo "unsupported"; exit 3; }
docker_apt_repository "$id" "$codename" >/dev/null
read -ra packages < <(docker_apt_packages "$id" "$version" "$codename")
docker_apt_install --dry-run "${packages[@]}" 2>/dev/null |
  awk '$1 == "Inst" && $2 ~ /^(docker-ce|docker-ce-cli|containerd\.io|docker-compose-plugin)$/ { print $2, substr($3, 2) }'
EOF
)"

# shellcheck source=SCRIPTDIR/../../packaging/docker-apt.sh
. "$LIB"
for spec in $DOCKER_SUPPORTED; do
  IFS=: read -r id version _ <<<"$spec"
  name="$RUN-$id-${version//./}"
  NAMES+=("$name")
  if out="$(docker run --rm --name "$name" -v "$LIB:/qs/docker-apt.sh:ro" "$id:$version" bash -c "$INSIDE" 2>/dev/null)" &&
    [[ "$(grep -c . <<<"$out")" -eq 4 ]]; then
    record PASS "$id $version" "$(tr '\n' ' ' <<<"$out" | sed 's/ $//')"
  else
    record FAIL "$id $version" "$(tail -3 <<<"$out" | tr '\n' ' ')"
  fi
done

name="$RUN-ubuntu-2004"
NAMES+=("$name")
rc=0
docker run --rm --name "$name" -v "$LIB:/qs/docker-apt.sh:ro" ubuntu:20.04 bash -c "$INSIDE" >/dev/null 2>&1 || rc=$?
if ((rc == 3)); then
  record PASS "ubuntu 20.04 refused"
else
  record FAIL "ubuntu 20.04 refused" "exit $rc"
fi

echo
echo "Summary:"
printf '  %s\n' "${RESULTS[@]}"
if ((FAILED)); then
  echo "Result: FAIL"
  exit 1
fi
echo "Result: PASS"
