#!/usr/bin/env bash
set -euo pipefail
image=$(realpath "${1:?AppImage path required}")
test -x "$image"
APPIMAGE_EXTRACT_AND_RUN=1 "$image" --version
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT
cd "$temporary"
"$image" --appimage-extract >/dev/null
desktop-file-validate squashfs-root/com.vitalyart.airpods-driver.desktop
test -f squashfs-root/com.vitalyart.airpods-driver.png
./squashfs-root/AppRun --version
