# Walkie-Textie Hub firmware

ESP32-S3 + Wio-SX1262 hub firmware (Rust/Embassy), forked from the node
firmware `walkie-textie-rust-firmware` (kept as the `upstream` git remote so
driver fixes can be merged). Two config-selected modes share one radio:
bridge (LoRa <-> MQTT over WiFi) and single-channel LoRaWAN gateway (Semtech
UDP forwarder for ChirpStack). No C anywhere; no BLE (the node firmware has
it, this fork removed it for WiFi). The PCB design stays in the node repo.

## Architecture

- `src/main.rs` boots, loads config (flash `nvs` partition overrides `HUB_*`
  build-time env defaults, reboot-to-apply), then spawns tasks by mode. The
  radio has exactly one owner: `lora_task` in bridge mode, `gateway_task` in
  gateway mode (the dispatcher then runs against `NullRadio` so serial
  commands still answer).
- `src/dispatcher/` routes commands/responses between serial, the radio and
  WiFi sources (channels: `COMMAND_CHANNEL`, `RESPONSE_CHANNEL` pub-sub,
  `HUB_CHANNEL` for provisioning).
- `src/lora/` is the hand-written SX1262 driver. RX is split into cancel-safe
  arm/wait/read phases (only `wait_rx_event` may be raced in a select; it
  returns the RX `Instant` that anchors LoRaWAN downlink timing). TX splits
  into `prepare_tx`/`fire_tx`/`wait_tx_done` for precise downlink windows.
  `LoraConfig` carries sync word (private/public), IQ inversion, CRC and
  preamble; defaults are byte-identical to the node firmware (regression KAT).
- `src/bridge/` is the pure LoRa<->MQTT state machine; `src/gateway/` the
  pure forwarder (UDP codec, acceptance policy, tmst scheduler). Both are
  host-tested; their embassy channels are embedded-gated.
- `src/tasks/` are thin shells: serial, lora, wifi (reconnect+backoff), mqtt
  (rust-mqtt over embassy-net TCP; `poll_header` is the only cancel-safe
  point), bridge, gateway, gateway_udp, hub_ctrl, admin, led.
- `hub-protocol/` is the in-repo crate for provisioning commands (ids
  0x20-0x2F) layered on the untouched `vendor/wt-protocol` submodule framing;
  the serial reader parses stock commands first and falls through on unknown
  ids. Keep hub-only commands here, never in wt-protocol.
- `src/hub_config/` holds the config struct (serde/postcard) and the flash
  store (esp-storage -> partition table lookup -> sequential-storage map).

Pin map (unchanged from the node board): SCLK GPIO7, MISO GPIO8, MOSI GPIO9,
NSS GPIO41, DIO1 GPIO39, NRST GPIO42, BUSY GPIO40, LED GPIO48 (active low),
USB D+ GPIO20 / D- GPIO19. SPI2 at 1 MHz Mode 0.

## Required checks after ANY firmware change

1. `cargo test --features host-test --target x86_64-unknown-linux-gnu` (full
   suite) plus `cargo test --manifest-path hub-protocol/Cargo.toml --target
   x86_64-unknown-linux-gnu` and the same for `vendor/wt-protocol`.
2. `cargo clippy --target x86_64-unknown-linux-gnu --features host-test
   --all-targets -- -D warnings`.
3. `cargo +esp build --features embedded --release -Zbuild-std=core,alloc`
   must be warning-clean (and `cargo +esp clippy ...` for code changes).
4. Hardware tests matching what changed: `cargo integration` (serial),
   `cargo lora` (two boards OTA), `cargo duty` (duty cycle), `cargo hub`
   (bridge mode end-to-end, needs a node board + mosquitto), `cargo gateway`
   (forwarder protocol against a host UDP listener).

## Gotchas

- Power-cycle after every flash: the board stays in ROM download mode until
  unplugged (native USB-Serial-JTAG). A running hub shows USB serial
  `WTH-XXXXXX`; the node firmware shows `WT-XXXXXX` - the hub integration
  tests tell boards apart by that prefix.
- Debug logs stream on the second CDC port (interface 2, 115200): WiFi/MQTT
  state changes, `LoRa RX/TX`, `Gateway ...` lines. The port re-enumerates on
  every reboot.
- Dependency versions are pinned deliberately: esp-storage 0.8 matches
  esp-hal 1.0 (0.9 needs 1.1-rc), embassy-embedded-hal 0.5 and embassy-net
  0.8 match embassy-sync 0.7 / embedded-io-async 0.7. Check alignment before
  bumping any of them.
- The wire layers must not mix: host-link framing lives in wt-protocol /
  hub-protocol; MQTT payloads are JSON in `src/bridge/codec.rs`; the gateway
  speaks Semtech UDP JSON in `src/gateway/udp_protocol.rs`.
- Mode switching is reboot-only by design (one radio, incompatible radio
  parameters between modes).
- Gateway mode is a single-channel forwarder: one frequency/SF, deaf during
  TX, devices must be channel-pinned with ADR off. It is payload-agnostic -
  never add a LoRaWAN crypto crate to this firmware.
- Duty cycle budgets (bridge TX and gateway downlinks) are RAM-only and reset
  on reboot; the `cargo duty` test relies on that.
