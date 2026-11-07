#!/usr/bin/env bash
# Verify that the installed application creates its main window with software graphics.
set -euo pipefail
temporary=$(mktemp -d)
gui_pid=
xvfb_pid=
cleanup() {
    if [[ -n "$gui_pid" ]]; then kill "$gui_pid" 2>/dev/null || true; fi
    if [[ -n "$xvfb_pid" ]]; then kill "$xvfb_pid" 2>/dev/null || true; fi
    rm -rf "$temporary"
}
trap cleanup EXIT
export DISPLAY=:98
export LIBGL_ALWAYS_SOFTWARE=1
export XDG_RUNTIME_DIR="$temporary/runtime"
mkdir -m700 "$XDG_RUNTIME_DIR"
Xvfb "$DISPLAY" -screen 0 1280x800x24 -ac -nolisten tcp >"$temporary/xvfb.log" 2>&1 &
xvfb_pid=$!
for attempt in {1..40}; do
    if xwininfo -root >/dev/null 2>&1; then break; fi
    sleep 0.25
done
timeout 30s dbus-run-session -- "$@" >"$temporary/application.log" 2>&1 &
gui_pid=$!
for attempt in {1..80}; do
    if xwininfo -name "Airpods Driver" -stats 2>/dev/null | grep -q 'Map State: IsViewable'; then
        sleep 1
        if ! kill -0 "$gui_pid" 2>/dev/null; then break; fi
        echo 'Airpods Driver main window appeared successfully'
        exit 0
    fi
    if ! kill -0 "$gui_pid" 2>/dev/null; then break; fi
    sleep 0.25
done
cat "$temporary/xvfb.log" "$temporary/application.log" >&2
echo 'Airpods Driver did not create its main window' >&2
exit 1
