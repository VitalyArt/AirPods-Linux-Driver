#!/usr/bin/env bash
# Invoked by package-docker.sh in the dedicated packaging image.
set -euo pipefail
umask 022
mkdir -p /work
cd /work
# Copy only build inputs: host target/, Cargo configuration and credentials stay out.
cp -a /source/{Cargo.toml,Cargo.lock,LICENSE,README.md,src,assets,docs,packaging,scripts,tests} .
python3 scripts/packaging/prepare-release.py
mkdir dist
export_packages() {
    local package mode
    for package in dist/*.deb dist/*.rpm dist/*.AppImage dist/*.flatpak; do
        [[ -f "$package" ]] || continue
        mode=644
        [[ "$package" != *.AppImage ]] || mode=755
        install -m "$mode" -o "$OUTPUT_UID" -g "$OUTPUT_GID" "$package" /out/
    done
}
trap export_packages EXIT
export FLATPAK_GL_DRIVERS=default
format=${1:-all}
if [[ "$format" == all || "$format" == flatpak ]]; then
    mkdir -p /run/dbus
    dbus-daemon --system --fork --nopidfile
    flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
    flatpak config --user --set languages en
fi
case "$format" in
    all) dbus-run-session -- python3 scripts/packaging/build-packages.py ;;
    deb) python3 scripts/packaging/build-deb.py ;;
    rpm|appimage) python3 scripts/packaging/build-linux.py "$format" ;;
    flatpak) dbus-run-session -- python3 scripts/packaging/build-linux.py flatpak ;;
    test)
        python3 -m unittest discover -s tests/packaging -v
        cargo test --locked
        desktop-file-validate packaging/desktop/com.vitalyart.airpods-driver.desktop
        ;;
    *) echo "Unknown packaging command: $format" >&2; exit 2 ;;
esac
if [[ "$format" == all || "$format" == flatpak ]]; then
    dbus-run-session -- bash scripts/verify/verify-flatpak.sh dist/*.flatpak
    dbus-run-session -- bash scripts/verify/verify-flatpak-update.sh
fi
