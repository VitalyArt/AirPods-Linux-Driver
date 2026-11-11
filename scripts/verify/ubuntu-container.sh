#!/usr/bin/env bash
# Resolve Ubuntu mirrors on the runner, outside the container DNS environment.
set -euo pipefail
options=()
for mirror in archive.ubuntu.com security.ubuntu.com ports.ubuntu.com; do
    if ! address=$(getent ahostsv4 "$mirror" | awk 'NR == 1 {print $1}') || [[ -z "$address" ]]; then
        echo "Runner DNS cannot resolve $mirror; trying DNS over HTTPS" >&2
        address=$(curl --fail --silent --show-error --retry 3 \
            --connect-timeout 10 --max-time 30 \
            -H "accept: application/dns-json" \
            "https://1.1.1.1/dns-query?name=$mirror&type=A" |
            python3 -c '
import ipaddress, json, sys
response = json.load(sys.stdin)
if response.get("Status") != 0:
    raise SystemExit("DNS-over-HTTPS lookup failed")
for answer in response.get("Answer", []):
    if answer.get("type") == 1:
        address = ipaddress.IPv4Address(answer["data"])
        if address.is_global:
            print(address)
            break
else:
    raise SystemExit("DNS-over-HTTPS returned no public IPv4 address")
')
    fi
    options+=(--add-host "$mirror:$address")
done
docker run --rm "${options[@]}" "$@"
