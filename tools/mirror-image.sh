#!/usr/bin/env bash
# Mirror a third-party image into the installation's ECR registry, by digest
# (decisions 0002 and 0003): the copy keeps the very digest of the source, so
# the reference the agent pins is the same in ECR. Run by a person, with AWS
# credentials that may push to the repository (Terraform creates it).
#
#   tools/mirror-image.sh <registry> [<source@sha256:...> <repository> <tag>]
#
#   <registry>    123456789012.dkr.ecr.us-east-1.amazonaws.com (any other
#                 registry is used as it is, without logging in: a local one
#                 to try the copy)
#   defaults:     the agent's helper image, busybox 1.37.0, into
#                 quality-data/tools/busybox:1.37.0
#
# Needs docker with buildx and, for ECR, the AWS CLI (AWS_PROFILE honoured).
set -euo pipefail

registry=${1:?usage: tools/mirror-image.sh <registry> [<source@sha256:...> <repository> <tag>]}
source=${2:-docker.io/library/busybox@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec}
repository=${3:-quality-data/tools/busybox}
tag=${4:-1.37.0}

digest=${source##*@}
if [[ ! $digest =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "error: the source must be pinned by digest (<image>@sha256:...)" >&2
  exit 2
fi

config=$(mktemp -d)
trap 'rm -rf "$config"' EXIT
export DOCKER_CONFIG=$config
# buildx is a CLI plugin: keep the user's plugins in reach.
for dir in "$HOME/.docker/cli-plugins" /usr/local/lib/docker/cli-plugins /usr/libexec/docker/cli-plugins; do
  if [[ -d $dir ]]; then
    mkdir -p "$config/cli-plugins"
    cp -a "$dir"/. "$config/cli-plugins/"
  fi
done

if [[ $registry =~ ^[0-9]{12}\.dkr\.ecr\.([a-z0-9-]+)\.amazonaws\.com$ ]]; then
  region=${BASH_REMATCH[1]}
  aws ecr get-login-password --region "$region" |
    docker login --username AWS --password-stdin "$registry" >/dev/null
fi

target="$registry/$repository:$tag"
echo "copying $source to $target"
docker buildx imagetools create --tag "$target" "$source"

copied=$(docker buildx imagetools inspect "$target" --format '{{json .Manifest}}' |
  grep -m 1 -o 'sha256:[0-9a-f]\{64\}' || true)
if [[ $copied != "$digest" ]]; then
  echo "FAIL: $target is $copied, not $digest" >&2
  exit 1
fi
echo "PASS: $registry/$repository@$digest"
