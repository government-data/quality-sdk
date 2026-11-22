# Docker from Docker's own apt repository, at pinned versions (decision 0007
# point 1). Sourced by install.sh (--install-docker) and by
# tools/verify/docker-apt.sh, which runs these same functions in throwaway
# containers of every supported distribution.
# shellcheck shell=bash disable=SC2034 # the settings are read by who sources it

# The primary key's fingerprint of download.docker.com's signing key; the
# downloaded key is trusted only when it matches.
DOCKER_KEY_FINGERPRINT=9DC858229FC7DD38854AE2D88D81803C0EBFCD88
DOCKER_REPOSITORY=https://download.docker.com/linux
# What --install-docker installs: docker-ce and docker-ce-cli, the Compose
# plugin and containerd.io, each with the distribution's suffix.
DOCKER_ENGINE_VERSION=29.10.0
DOCKER_COMPOSE_VERSION=5.8.0
DOCKER_CONTAINERD_VERSION=2.4.2-4
# id:version:codename, as /etc/os-release names them.
DOCKER_SUPPORTED="ubuntu:24.04:noble ubuntu:22.04:jammy debian:12:bookworm"
# The oldest Docker Engine and Compose plugin a tools/verify/ script passed
# with (decision 0005, Verification); older ones are refused.
DOCKER_FLOOR_ENGINE=29.4.1
DOCKER_FLOOR_COMPOSE=5.1.3

# docker_apt_distro OS_RELEASE: "id version codename" of a supported
# distribution; fails, printing nothing, for any other.
docker_apt_distro() {
  local found spec
  # shellcheck disable=SC1090 # the distribution's own file
  found="$(. "$1" && echo "${ID:-}:${VERSION_ID:-}:${VERSION_CODENAME:-}")" || return 1
  for spec in $DOCKER_SUPPORTED; do
    if [[ "$spec" == "$found" ]]; then
      echo "${found//:/ }"
      return 0
    fi
  done
  return 1
}

# docker_apt_packages ID VERSION CODENAME: the pinned package=version list.
docker_apt_packages() {
  local suffix="~$1.$2~$3"
  echo "docker-ce=5:$DOCKER_ENGINE_VERSION-1$suffix" \
    "docker-ce-cli=5:$DOCKER_ENGINE_VERSION-1$suffix" \
    "containerd.io=$DOCKER_CONTAINERD_VERSION$suffix" \
    "docker-compose-plugin=$DOCKER_COMPOSE_VERSION-1$suffix"
}

# docker_apt_repository ID CODENAME: Docker's repository added, its key
# checked against the pinned fingerprint first. Root's own keyring is not
# touched; the files apt reads are 0644 (apt reads them as _apt).
docker_apt_repository() {
  local id=$1 codename=$2 home key fingerprint arch
  DEBIAN_FRONTEND=noninteractive apt-get update -qq
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq ca-certificates curl gnupg >/dev/null
  home="$(mktemp -d)"
  key="$home/docker.asc"
  if ! curl -fsSL "$DOCKER_REPOSITORY/$id/gpg" -o "$key"; then
    rm -rf "$home"
    echo "error: could not download Docker's signing key" >&2
    return 1
  fi
  fingerprint="$(GNUPGHOME="$home" gpg --batch --show-keys --with-colons "$key" 2>/dev/null |
    awk -F: '$1 == "fpr" { print $10; exit }')"
  if [[ "$fingerprint" != "$DOCKER_KEY_FINGERPRINT" ]]; then
    rm -rf "$home"
    echo "error: Docker's signing key is not the pinned one (${fingerprint:-none})" >&2
    return 1
  fi
  install -d -m 0755 /etc/apt/keyrings
  GNUPGHOME="$home" gpg --batch --dearmor <"$key" >/etc/apt/keyrings/docker.gpg
  rm -rf "$home"
  chmod 0644 /etc/apt/keyrings/docker.gpg
  arch="$(dpkg --print-architecture)"
  echo "deb [arch=$arch signed-by=/etc/apt/keyrings/docker.gpg] $DOCKER_REPOSITORY/$id $codename stable" \
    >/etc/apt/sources.list.d/docker.list
  chmod 0644 /etc/apt/sources.list.d/docker.list
  DEBIAN_FRONTEND=noninteractive apt-get update -qq
}

# docker_apt_install [--dry-run] PACKAGE=VERSION...: install them.
docker_apt_install() {
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "$@"
}

# docker_version_at_least HAVE FLOOR: whether version HAVE >= FLOOR
# (dotted numbers, a leading v and anything after the numbers ignored).
docker_version_at_least() {
  local have=${1#v} floor=${2#v}
  have=${have%%[!0-9.]*} floor=${floor%%[!0-9.]*}
  [[ -n "$have" ]] || return 1
  [[ "$(printf '%s\n%s\n' "$floor" "$have" | sort -V | head -1)" == "$floor" ]]
}
