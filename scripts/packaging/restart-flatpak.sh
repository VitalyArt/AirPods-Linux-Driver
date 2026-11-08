# Executed on the host via sh -c; all variable arguments are passed positionally.
set -eu
instance=$1
installation=$2
app=$3
shift 3
command -v flatpak >/dev/null
flatpak info "$installation" "$app" >/dev/null
printf 'ready\n'
# Stop writing to the readiness pipe before the old sandbox closes it.
exec >/dev/null 2>&1
# Waiting for the old instance also releases its single-instance lock and tray.
attempt=0
while [ "$attempt" -lt 300 ]; do
    running=$(flatpak ps --columns=instance) || exit 1
    case "
$running
" in
        *"
$instance
"*) sleep 0.1 ;;
        *) exec flatpak run "$installation" "$app" "$@" ;;
    esac
    attempt=$((attempt + 1))
done
exit 1
