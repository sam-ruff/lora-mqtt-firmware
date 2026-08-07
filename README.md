# LoRaMqttHub

Firmware that turns the Walkie-Textie radio board (ESP32-S3 + Wio-SX1262) into
a mains-powered hub. It has two operating modes, selected by configuration:

- **Bridge**: receives Walkie-Textie LoRa packets and publishes them to an
  MQTT broker over WiFi, and transmits packets published to its command topic.
- **LoRaWAN gateway**: a single-channel Semtech UDP packet forwarder for a
  LoRaWAN network server such as ChirpStack.

Pure Rust, no C: esp-hal + esp-rtos (Embassy executor), esp-radio for WiFi,
a hand-written SX1262 driver, embassy-net and rust-mqtt. The shared wire
protocol lives in the [`wt-protocol`](https://github.com/sam-ruff/walkie-textie-protocol)
submodule, so clone with `--recursive`.

## Building and flashing

```bash
cargo install espup espflash
espup install
source "$HOME/export-esp.sh"

cargo +esp build --features embedded --release -Zbuild-std=core,alloc
./flash_devices.sh          # build + flash all connected boards
```

After flashing, power-cycle the board (unplug and replug, do not hold BOOT).
The ESP32-S3 native USB-Serial-JTAG cannot be reset into the application from
the host, so the board stays in ROM download mode until power-cycled. A
running hub enumerates as "LoRaMqttHub" with USB serial `LMH-XXXXXX`
and exposes two CDC ports: data (interface 0) and debug log (interface 2).

## Configuration

An unprovisioned hub (no WiFi credentials) starts its own open hotspot,
`LoRaMqttHub-XXXXXX`. Join it with a phone and the captive portal at
`http://192.168.4.1` opens: set the WiFi network, MQTT broker, operating mode
and gateway settings, save, and the hub restarts onto your network. The same
portal appears again if the configured network stays unreachable (wrong
password, network gone), so a hub can always be re-provisioned without a
cable. The portal also serves a JSON API (`GET/POST /api/config`,
`GET /api/status`) for app-driven setup.

Defaults can also be baked at build time through environment variables, all
optional: `HUB_WIFI_SSID`, `HUB_WIFI_PASSWORD`, `HUB_MQTT_HOST`,
`HUB_MQTT_PORT`, `HUB_MQTT_CLIENT_ID`, `HUB_MODE` (`bridge` or `gateway`),
`HUB_GW_HOST`, `HUB_GW_PORT`.

Runtime provisioning also works over the USB data port with hub commands
layered on the Walkie-Textie wire protocol (COBS-framed, CRC-16, see
`hub-protocol/`): set WiFi credentials, MQTT broker, gateway settings and
mode; read back the configuration (the WiFi password is never echoed) and
live status (link states, IP, counters, uptime). Settings persist in the
`nvs` flash partition and apply on the next boot, so the flow is set, then
reboot.

## Bridge mode

Topics live under `wt/hub/<id>/` where `<id>` is the hex device id from the
USB serial:

- `rx`: received LoRa packets, JSON with `payload_hex`, `rssi`, `snr`, `seq`
  and `uptime_ms`.
- `tx`: publish `{"payload_hex":"...","id":123}` here to transmit over LoRa;
  `id` is optional and echoed back.
- `tx/result`: `{"result":"sent"|"refused"|"error"...}` per downlink, with
  `retry_after_secs` when the EU duty cycle budget refuses a transmission.
- `status`: retained `{"online":true,...}`, with an offline last-will.

The broker connection is plain TCP (local broker assumed); the client id
defaults to `wt-hub-<id>`.

## LoRaWAN gateway mode

Speaks the Semtech UDP packet-forwarder protocol to a network server -
typically the ChirpStack gateway bridge on port 1700. The gateway EUI derives
from the MAC address with the standard FFFE insertion.

An SX1262 is a single-channel, half-duplex radio, which makes this a
single-channel gateway with real limitations:

- Only uplinks on the configured frequency and spreading factor are heard
  (default 868.1 MHz, SF7BW125). OTAA joins hop across three channels, so
  pin devices to the gateway channel and disable ADR in the device profile.
- The radio is deaf while transmitting a downlink.
- The Things Network discourages single-channel gateways; use a self-hosted
  ChirpStack.

Downlinks obey the EU duty cycle per sub-band and are refused with a
`DUTY_CYCLE_OVERFLOW` TX_ACK when the hourly budget is spent. The gateway is
payload-agnostic: joins, MICs and encryption all happen at the network server.

## Configurator app

`app/` holds a Flutter app for phones that drives the same provisioning API
as the captive portal: join the hub's hotspot, open the app, connect and set
everything up. The HTTP client and validation live in a Rust engine
(`app/rust_backend/`) exposed through flutter_rust_bridge, mirroring the
Walkie-Textie messenger app's architecture: `./build_rust.sh` regenerates the
bindings and builds the engine, Android cross-compiles it via cargo-ndk from
Gradle, and iOS links a static xcframework built by `tool/build_ios_libs.sh`.

```bash
cd app
./build_rust.sh
flutter run --dart-define=MOCK=true    # full flow against a fake hub, no hardware
flutter run                            # against a real hub
```

## Testing

Host tests need no hardware:

```bash
cargo test --features host-test --target x86_64-unknown-linux-gnu
cargo test --manifest-path hub-protocol/Cargo.toml --target x86_64-unknown-linux-gnu
cargo test --manifest-path vendor/wt-protocol/Cargo.toml --target x86_64-unknown-linux-gnu
```

Hardware tests run from `integration_tests/` against flashed boards:

```bash
cargo integration     # serial host-link, one board
cargo lora            # two boards over the air
cargo duty            # duty cycle lockout (spends ~6 min of airtime)
cargo hub -- --wifi-ssid <ssid> --wifi-password <pw> --broker-host <ip>
cargo gateway -- --server-host <this machine's LAN IP>
```

`cargo hub` needs a hub board, a node board and a reachable MQTT broker; it
provisions the hub, then proves both bridge directions end to end.
`cargo gateway` binds the network-server side itself and verifies the
forwarder protocol against the live gateway. Watch the debug CDC port
(115200 baud) for the firmware's own view of events.
