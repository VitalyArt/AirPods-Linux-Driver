#!/usr/bin/env bash
set -euo pipefail
bundle=$(realpath "${1:?Flatpak bundle path required}")
app=com.vitalyart.airpods-driver
# Install from the actual downloaded file, not from the build repository.
flatpak install --user --noninteractive --assumeyes "$bundle"
flatpak info --user "$app"
flatpak run --user "$app" --version
if command -v Xvfb >/dev/null; then
    bash scripts/verify/verify-gui.sh flatpak run --user "$app"
fi
location=$(flatpak info --user --show-location "$app")
test -f "$location/export/share/applications/$app.desktop"
test -f "$location/export/share/icons/hicolor/256x256/apps/$app.png"
flatpak uninstall --user --noninteractive --assumeyes "$app"
