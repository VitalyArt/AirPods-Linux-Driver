#!/usr/bin/env bash
# Run after installation, inside the test distribution (not on the developer host).
set -euo pipefail
airpods-driver --version
test -x /usr/bin/airpods-driver
test -f /usr/share/applications/com.vitalyart.airpods-driver.desktop
test -f /usr/share/icons/hicolor/256x256/apps/com.vitalyart.airpods-driver.png
desktop-file-validate /usr/share/applications/com.vitalyart.airpods-driver.desktop
if ! linked=$(ldd /usr/bin/airpods-driver 2>&1); then
    printf '%s\n' "$linked" >&2
    echo 'Failed to inspect shared libraries' >&2
    exit 1
fi
if grep -q 'not found' <<<"$linked"; then
    echo 'Unresolved shared libraries' >&2
    exit 1
fi
