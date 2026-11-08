#!/usr/bin/env bash
# Run only in an isolated packaging container with a Freedesktop runtime installed.
# Uses a separate fixture ID; never installs over the real application.
set -euo pipefail
export FLATPAK_GL_DRIVERS=default
app=com.vitalyart.airpods-driver-update-test
scope=${1:---user}
work=$(mktemp -d)
cleanup() {
    flatpak uninstall "$scope" --noninteractive --assumeyes "$app" >/dev/null 2>&1 || true
    rm -rf "$work"
}
trap cleanup EXIT
runtime=org.freedesktop.Platform
branch=25.08
stage="$work/stage"
repo="$work/repo"
flatpak build-init "$stage" "$app" "$runtime" "$runtime" "$branch"
mkdir -p "$stage/files/bin" "$stage/files/share"
cp "$(dirname "$0")/../packaging/restart-flatpak.sh" "$stage/files/share/restart-flatpak.sh"
cat > "$stage/files/bin/update-test" <<'APP'
#!/bin/sh
set -eu
if [ "${1:-}" = --version ]; then cat /app/share/version; exit; fi
if [ "${1:-}" = --restart-check ]; then
    cat /app/share/version > "$XDG_CACHE_HOME/restarted-version"
    exit
fi
grep '^app-path=' /.flatpak-info
scope=--user
if grep -q '^app-path=/var/lib/flatpak/app/' /.flatpak-info; then scope=--system; fi
mkdir -p "$XDG_CACHE_HOME/updates"
cp /app/share/next.flatpak "$XDG_CACHE_HOME/updates/next.flatpak"
flatpak-spawn --host --watch-bus --env=FLATPAK_GL_DRIVERS=default flatpak install "$scope" --bundle --or-update --noninteractive --assumeyes "$XDG_CACHE_HOME/updates/next.flatpak"
version=$(flatpak-spawn --host --watch-bus --env=FLATPAK_GL_DRIVERS=default flatpak run "$scope" com.vitalyart.airpods-driver-update-test --version)
test "$version" = new
echo 'PASS: bundle update from inside the sandbox installed the new version'
instance=$(sed -n 's/^instance-id=//p' /.flatpak-info)
flatpak-spawn --host --env=FLATPAK_GL_DRIVERS=default sh -c "$(cat /app/share/restart-flatpak.sh)" \
    restart-flatpak "$instance" "$scope" com.vitalyart.airpods-driver-update-test --restart-check \
    > "$XDG_CACHE_HOME/restart-ready" &
launcher=$!
attempt=0
while [ "$attempt" -lt 100 ]; do
    if grep -q '^ready$' "$XDG_CACHE_HOME/restart-ready" 2>/dev/null; then
        kill -KILL "$launcher"
        wait "$launcher" 2>/dev/null || true
        exit 0
    fi
    sleep 0.1
    attempt=$((attempt + 1))
done
exit 1
APP
chmod +x "$stage/files/bin/update-test"
echo new > "$stage/files/share/version"
flatpak build-finish --command=update-test --talk-name=org.freedesktop.Flatpak "$stage"
flatpak build-export "$repo" "$stage"
flatpak build-bundle "$repo" "$work/new.flatpak" "$app"
echo old > "$stage/files/share/version"
cp "$work/new.flatpak" "$stage/files/share/next.flatpak"
flatpak build-export "$repo" "$stage"
flatpak build-bundle "$repo" "$work/old.flatpak" "$app"
flatpak install "$scope" --bundle --noninteractive --assumeyes "$work/old.flatpak"
rm -f "$HOME/.var/app/$app/cache/restarted-version" "$HOME/.var/app/$app/cache/restart-ready"
test "$(flatpak run "$scope" "$app" --version)" = old
flatpak run "$scope" "$app"
for attempt in {1..150}; do
    if [[ -f "$HOME/.var/app/$app/cache/restarted-version" ]]; then break; fi
    sleep 0.1
done
test "$(cat "$HOME/.var/app/$app/cache/restarted-version")" = new
echo 'PASS: updated Flatpak restarted automatically after the old instance exited'
test "$(flatpak run "$scope" "$app" --version)" = new
flatpak uninstall "$scope" --noninteractive --assumeyes "$app"
