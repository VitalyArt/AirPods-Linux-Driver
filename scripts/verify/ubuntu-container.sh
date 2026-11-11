#!/usr/bin/env bash
# Resolve Ubuntu mirrors on the runner, outside the container DNS environment.
set -euo pipefail
options=()
for mirror in archive.ubuntu.com security.ubuntu.com ports.ubuntu.com; do
    if ! address=$(getent ahostsv4 "$mirror" | awk 'NR == 1 {print $1}') || [[ -z "$address" ]]; then
        echo "Cannot resolve Ubuntu mirror on the runner: $mirror" >&2
        exit 1
    fi
    options+=(--add-host "$mirror:$address")
done
docker run --rm "${options[@]}" "$@"
