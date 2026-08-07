#!/bin/bash
# Drive the full portal e2e flow against a real unprovisioned hub:
# join the hub's hotspot, run the read-only portal tests on desktop and
# mobile viewports, provision it through the UI, then rejoin the home
# network.
#
# Required env: HUB_WIFI_SSID, HUB_WIFI_PASSWORD (the network the hub should
# join), HUB_MQTT_HOST (this machine's LAN address for the broker).
# Optional: WIFI_IFACE (default: first wifi device), HOME_CONNECTION
# (nmcli connection to restore, default: the current one on the interface).

set -euo pipefail
cd "$(dirname "$0")"

: "${HUB_WIFI_SSID:?set HUB_WIFI_SSID}"
: "${HUB_WIFI_PASSWORD:?set HUB_WIFI_PASSWORD}"
: "${HUB_MQTT_HOST:?set HUB_MQTT_HOST}"

WIFI_IFACE="${WIFI_IFACE:-$(nmcli -t -f DEVICE,TYPE device status | awk -F: '$2=="wifi"{print $1; exit}')}"
HOME_CONNECTION="${HOME_CONNECTION:-$(nmcli -t -f NAME,DEVICE connection show --active | awk -F: -v d="$WIFI_IFACE" '$2==d{print $1; exit}')}"

echo "Scanning for the hub hotspot on $WIFI_IFACE..."
HUB_AP=""
for _ in $(seq 1 12); do
    HUB_AP=$(nmcli -t -f SSID device wifi list ifname "$WIFI_IFACE" --rescan yes 2>/dev/null \
        | grep '^WalkieTextieHub-' | head -1 || true)
    [ -n "$HUB_AP" ] && break
    sleep 5
done
if [ -z "$HUB_AP" ]; then
    echo "No WalkieTextieHub-* hotspot found. Is the hub powered and unprovisioned?"
    exit 1
fi
echo "Joining $HUB_AP..."

restore_network() {
    if [ -n "$HOME_CONNECTION" ]; then
        echo "Rejoining $HOME_CONNECTION..."
        nmcli connection up id "$HOME_CONNECTION" ifname "$WIFI_IFACE" >/dev/null || true
    fi
}
trap restore_network EXIT

nmcli device wifi connect "$HUB_AP" ifname "$WIFI_IFACE"

echo "Waiting for the portal..."
for _ in $(seq 1 20); do
    if curl -fsS -m 2 -o /dev/null http://192.168.4.1/api/status; then
        break
    fi
    sleep 1
done
curl -fsS -m 2 -o /dev/null http://192.168.4.1/api/status

echo "Running portal tests (desktop and mobile)..."
npx playwright test portal

echo "Provisioning the hub through the UI..."
PROVISION=1 npx playwright test provision --project=desktop

echo "Provisioned. The hub is rebooting onto $HUB_WIFI_SSID."
