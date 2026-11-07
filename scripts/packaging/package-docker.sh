#!/usr/bin/env bash
set -euo pipefail
format=${1:-all}
case "$format" in all|deb|rpm|appimage|flatpak|test) ;; *) echo "Unknown packaging command: $format" >&2; exit 2 ;; esac
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
image=${PACKAGING_IMAGE:-airpods-driver-packaging:ubuntu24}
command -v docker >/dev/null || { echo 'Docker is required to build packages.' >&2; exit 1; }
docker info >/dev/null
if [[ "${PACKAGING_SKIP_IMAGE_BUILD:-0}" != 1 ]]; then
    docker build -f "$root/scripts/packaging/packaging.Dockerfile" -t "$image" "$root"
fi
arch=$(docker image inspect --format '{{.Architecture}}' "$image")
mkdir -p "$root/dist"
cache_mounts=()
if [[ -n "${PACKAGING_CACHE_DIR:-}" ]]; then
    mkdir -p "$PACKAGING_CACHE_DIR"
    cache_root=$(cd -- "$PACKAGING_CACHE_DIR" && pwd)
    for entry in "target-$arch:/build-target" "registry:/opt/cargo/registry" \
        "git:/opt/cargo/git" "flatpak-$arch:/root/.local/share/flatpak"; do
        directory=${entry%%:*}
        destination=${entry#*:}
        mkdir -p "$cache_root/$directory"
        cache_mounts+=(--mount "type=bind,src=$cache_root/$directory,dst=$destination")
    done
else
    cache_mounts+=(
        --mount "type=volume,src=airpods-driver-package-target-$arch,dst=/build-target"
        --mount "type=volume,src=airpods-driver-package-registry,dst=/opt/cargo/registry"
        --mount "type=volume,src=airpods-driver-package-git,dst=/opt/cargo/git"
        --mount "type=volume,src=airpods-driver-package-flatpak-$arch,dst=/root/.local/share/flatpak"
    )
fi
options=()
if [[ "$format" == all || "$format" == flatpak ]]; then
    # Flatpak needs nested sandbox namespaces inside this dedicated container.
    options+=(--privileged --security-opt apparmor=unconfined)
fi
docker run --rm "${options[@]}" \
    --mount "type=bind,src=$root,dst=/source,readonly" \
    --mount "type=bind,src=$root/dist,dst=/out" \
    "${cache_mounts[@]}" \
    -e CARGO_TARGET_DIR=/build-target -e "RELEASE_TAG=${RELEASE_TAG:-}" \
    -e "OUTPUT_UID=$(id -u)" -e "OUTPUT_GID=$(id -g)" \
    "$image" bash /source/scripts/packaging/build-in-container.sh "$format"
