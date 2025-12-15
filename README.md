# Walkie-Textie Rust Firmware

ESP32-S3 firmware with WIO-SX1262 LoRa module using Embassy async runtime. Receives COBS-encoded binary commands over serial or BLE and supports LoRa TX/RX operations.

## Building

### Prerequisites

Install the ESP Rust toolchain:

```bash
cargo install espup
espup install
source ~/export-esp.sh
```

You must run `source ~/export-esp.sh` in each new terminal session before building.

### Host Tests

Run unit tests on your development machine:

```bash
cargo t
# or: cargo test --target x86_64-unknown-linux-gnu
```

### Embedded Build (ESP32-S3)

Debug build:

```bash
cargo +esp build --features embedded -Zbuild-std=core,alloc
```

Release build:

```bash
cargo +esp build --features embedded --release -Zbuild-std=core,alloc
```

### Flash

```bash
espflash flash --port /dev/ttyACM0 target/xtensa-esp32s3-none-elf/release/walkie-textie-rust-firmware
```

Or use cargo run (configured in `.cargo/config.toml`):

```bash
cargo +esp run --features embedded --release -Zbuild-std=core,alloc
```

To flash multiple devices at once:

```bash
./flash_devices.sh                           # Auto-detect and flash all
./flash_devices.sh /dev/ttyACM0 /dev/ttyACM2 # Flash specific ports
```

**If flashing fails**, put the device in bootloader mode:
1. Hold the **BOOT** button
2. Press and release **RESET**
3. Release **BOOT** button
4. Run the flash command again

**After flashing**, unplug and replug the USB cable. Verify the device is running by checking that two serial ports appear:

```bash
ls /dev | grep ACM
# Should show: ttyACM0 ttyACM1 (for one device)
# Or: ttyACM0 ttyACM1 ttyACM2 ttyACM3 (for two devices)
```

### Monitor

The firmware exposes two USB CDC-ACM serial ports:
- **CDC0** (e.g. `/dev/ttyACM0`): Data port for commands/responses
- **CDC1** (e.g. `/dev/ttyACM1`): Debug log output

To monitor the debug output after flashing:

```bash
# Open debug port (second ttyACM device)
picocom /dev/ttyACM1 -b 115200
```

Debug output includes startup messages and LoRa/BLE events:
```
Walkie-Textie v0.1.0 starting...
Device ID: A1B2C3
Starting tasks...
LoRa: Initialising radio...
LoRa: Radio initialised
BLE: Starting as 'WalkieTextie-A1B2C3'
BLE: Advertising...
All tasks started
LoRa TX: 'Hello World'
LoRa TX: Complete
LoRa RX: 'Reply' (RSSI: -45, SNR: 8)
BLE: Connected
BLE: Disconnected
```

To monitor both ports simultaneously, use two terminals or a tool like `tmux`:
```bash
# Terminal 1: Data port (for sending commands)
picocom /dev/ttyACM0 -b 115200
# Terminal 2: Debug port (for viewing logs)
picocom /dev/ttyACM1 -b 115200
# For a second device
picocom /dev/ttyACM3 -b 115200
```

## Bootloader

This firmware uses the ESP-IDF 2nd stage bootloader. The bootloader is pre-flashed on most ESP32-S3 development boards.

### Flashing the Bootloader from Scratch

If you need to flash the bootloader (e.g., on a new chip or after corruption):

1. Install espflash:
   ```bash
   cargo install espflash
   ```

2. Download the ESP-IDF bootloader binary for ESP32-S3 from the espflash releases or build from ESP-IDF.

3. Flash the bootloader and partition table:
   ```bash
   espflash write-bin 0x0 bootloader.bin --port /dev/ttyACM0
   espflash write-bin 0x8000 partition-table.bin --port /dev/ttyACM0
   ```

Alternatively, espflash can flash a complete image including bootloader:
```bash
espflash flash --port /dev/ttyACM0 --bootloader bootloader.bin --partition-table partition-table.bin target/xtensa-esp32s3-none-elf/release/walkie-textie-rust-firmware
```

### Building a Silent Bootloader

By default, the ESP-IDF bootloader outputs log messages on boot. To disable this, build a custom bootloader with logging disabled using the project in `bootloader/`:

1. Install ESP-IDF (v5.2 or later):
   ```bash
   mkdir -p ~/esp
   cd ~/esp
   git clone -b v5.2.2 --recursive https://github.com/espressif/esp-idf.git
   cd esp-idf
   ./install.sh esp32s3
   ```

2. Build the silent bootloader:
   ```bash
   source ~/esp/esp-idf/export.sh
   cd bootloader
   idf.py set-target esp32s3
   idf.py build
   cp build/bootloader/bootloader.bin ../silent-bootloader-esp32s3.bin
   ```

3. Flash with the silent bootloader:
   ```bash
   espflash flash --port /dev/ttyACM0 \
       --bootloader silent-bootloader-esp32s3.bin \
       target/xtensa-esp32s3-none-elf/release/walkie-textie-rust-firmware
   ```

### Notes on ESP-IDF Bootloader Compatibility

The firmware includes an app descriptor (`esp_app_desc!` macro) required by the ESP-IDF bootloader for validation. The efuse block revision fields are set to accept all chip revisions (min=0, max=65535).

## Integration Tests

After flashing the firmware, run integration tests to verify functionality. Cargo aliases are provided for convenience and auto-detect data ports by default:

| Alias               | Description                |
|---------------------|----------------------------|
| `cargo integration` | Single-device serial tests |
| `cargo lora`        | Two-device LoRa tests      |
| `cargo ble-serial`  | BLE tests via serial       |
| `cargo ble-ble`     | BLE-to-BLE tests           |

Port auto-detection scans ttyACM devices and identifies the data port (CDC0) by sending a GetVersion command.

### Single-Device Tests

Tests basic command/response functionality:

```bash
# Auto-detect port (default)
cargo integration

# Or specify port manually
cargo integration --port /dev/ttyACM0
```

Options:
- `--port <PORT>`: Serial port (default: auto)
- `--baud <RATE>`: Baud rate (default: 115200)

The tests verify:
- GetVersion returns firmware version
- Invalid command returns error
- Multiple sequential commands work correctly

### Two-Device LoRa Tests

Tests bidirectional LoRa communication between two flashed devices:

```bash
# Auto-detect both ports (default)
cargo lora

# Or specify ports manually
cargo lora --port-a /dev/ttyACM0 --port-b /dev/ttyACM2
```

Options:
- `--port-a <PORT>`: Serial port for device A (default: auto)
- `--port-b <PORT>`: Serial port for device B (default: auto)
- `--baud <RATE>`: Baud rate (default: 115200)

The tests verify:
- A to B transmission
- B to A transmission
- Bidirectional ping-pong
- Multiple sequential messages
- Reliability (10 round trips)

## Hardware Configuration

| Pin    | Function         |
|--------|------------------|
| GPIO7  | SPI SCLK         |
| GPIO8  | SPI MISO         |
| GPIO9  | SPI MOSI         |
| GPIO41 | LoRa NSS (CS)    |
| GPIO39 | LoRa DIO1 (IRQ)  |
| GPIO42 | LoRa NRST        |
| GPIO40 | LoRa BUSY        |
| GPIO48 | LED (active low) |

TCXO voltage: 1.8V (configured via DIO3)

## Command Protocol

Binary protocol with COBS encoding and zero byte delimiter:

```
[COBS-encoded payload][0x00]

Payload: [version: u8][cmd_id: u8][length: u16 LE][data][crc16: u16 LE]
```

Protocol version is currently `1`. The firmware will reject commands with mismatched versions.

### Commands

| ID   | Command    | Payload              | Response   | Description                        |
|------|------------|----------------------|------------|------------------------------------|
| 0x01 | GetVersion | None                 | Version    | Returns firmware version           |
| 0x03 | Reboot     | None                 | None       | Reboots the device (no response)   |
| 0x10 | LoraTx     | Data bytes (max 256) | TxComplete | Transmits data over LoRa           |

### Responses

| ID   | Response   | Payload                          | Description                              |
|------|------------|----------------------------------|------------------------------------------|
| 0x01 | Version    | major, minor, patch (3 bytes)    | Firmware version response                |
| 0x10 | TxComplete | None                             | LoRa transmission completed successfully |
| 0x11 | RxPacket   | data, rssi (i16 LE), snr (i8)    | Received LoRa packet (unsolicited)       |
| 0xFF | Error      | status code, original command ID | Error response with status and cmd ID    |

### Response Format

Responses use the same frame structure as commands:

```
Payload: [version: u8][resp_id: u8][length: u16 LE][data][crc16: u16 LE]
```

### Unsolicited Responses

The firmware continuously listens for incoming LoRa packets in the background (100ms polling interval). When a packet is received, it is immediately pushed to the host as an unsolicited `RxPacket` response.

- Response ID: `0x11`
- Sequence ID: `0` (distinguishes unsolicited from request/response pairs)
- Payload: `[data bytes][rssi: i16 LE][snr: i8]`
- Max TX latency: 100ms (radio must exit RX mode to transmit)

The host must be ready to receive these at any time.

### Response Status Codes

| Code | Status         | Description                              |
|------|----------------|------------------------------------------|
| 0x00 | Success        | Command executed successfully            |
| 0x01 | InvalidCommand | Unknown command ID                       |
| 0x02 | InvalidLength  | Payload length invalid for command       |
| 0x03 | CrcError       | CRC-16 checksum mismatch                 |
| 0x04 | InvalidVersion | Protocol version mismatch                |
| 0x10 | LoraError      | LoRa radio error during operation        |
| 0x11 | Timeout        | Operation timed out                      |

### Example Frames

All examples show the complete COBS-encoded frame including the zero delimiter.

**GetVersion Command:**
```
Raw:    01 01 00 00 64 0b       (version=1, cmd=0x01, len=0, crc=0x0b64)
COBS:   03 01 01 01 03 64 0b 00
```

**GetVersion Response (v0.1.0):**
```
Raw:    01 01 03 00 00 01 00 15 b7    (version=1, resp=0x01, len=3, payload=[0,1,0], crc)
COBS:   04 01 01 03 05 01 01 15 b7 00
```

**Reboot Command:**
```
Raw:    01 03 00 00 e4 2f       (version=1, cmd=0x03, len=0, crc=0x2fe4)
COBS:   03 01 03 01 03 e4 2f 00
```

**LoraTx Command ("Hello"):**
```
Raw:    01 10 05 00 48 65 6c 6c 6f [crc16]
COBS:   [COBS-encoded bytes] 00
```

**Error Response (InvalidCommand for cmd 0xFE):**
```
Raw:    01 ff 02 00 01 fe [crc16]    (status=0x01, original_cmd=0xFE)
COBS:   [COBS-encoded bytes] 00
```

### CRC-16 Calculation

The CRC-16 uses the XMODEM polynomial (0x1021) with initial value 0x0000. The CRC is calculated over the bytes: `[version][cmd_id][length_lo][length_hi][payload...]`.

Python example:
```python
import crcmod
crc16 = crcmod.predefined.mkCrcFun('xmodem')
checksum = crc16(bytes([0x01, 0x01, 0x00, 0x00]))  # GetVersion: 0x0b64
```

### COBS Encoding

COBS (Consistent Overhead Byte Stuffing) encodes data to eliminate zero bytes, using 0x00 as frame delimiter. The firmware uses the `corncobs` implementation which appends the zero delimiter automatically.

Python example:
```python
from cobs import cobs
raw_frame = bytes([0x01, 0x01, 0x00, 0x00, 0x64, 0x0b])
encoded = cobs.encode(raw_frame) + b'\x00'  # Add delimiter
```

## Bluetooth LE

The firmware advertises as "WalkieTextie" and provides a Nordic UART Service (NUS) for command/response communication alongside serial.

### Nordic UART Service UUIDs

| Characteristic | UUID                                 |
|----------------|--------------------------------------|
| Service        | 6E400001-B5A3-F393-E0A9-E50E24DCCA9E |
| RX (write)     | 6E400002-B5A3-F393-E0A9-E50E24DCCA9E |
| TX (notify)    | 6E400003-B5A3-F393-E0A9-E50E24DCCA9E |

### Usage

1. Scan for and connect to "WalkieTextie"
2. Enable notifications on the TX characteristic
3. Write COBS-encoded commands to the RX characteristic
4. Receive COBS-encoded responses via TX notifications

The same binary protocol is used over BLE as over serial. Commands sent via BLE receive responses via BLE; unsolicited LoRa RX packets are only sent to serial.

Compatible apps: nRF Connect, any app supporting NUS.

## Architecture

The firmware uses esp-rtos with Embassy async tasks and channel-based communication:

- **Serial Reader Task**: Reads USB serial, parses COBS frames, sends commands to channel
- **Serial Writer Task**: Receives responses from channel, encodes and writes to USB serial
- **LoRa Task**: Continuously listens for LoRa packets (100ms polling), pushes received packets immediately to serial. Processes TX commands when available with max 100ms latency.
- **LED Task**: Flashes LED on TX/RX events via channel (non-blocking)
- **BLE Host Task**: Manages BLE advertising, connections, and Nordic UART Service. Routes commands to the same channel as serial.

Traits (`LoraRadio`, `SerialPort`) allow unit testing with mock implementations.

## CI/CD

GitHub Actions runs on every push to `main`:

1. **Test**: Runs unit tests (`cargo test`)
2. **Build**: Builds release firmware using ESP toolchain
3. **Release**: Creates GitHub releases via semantic-release

Releases are triggered by conventional commit messages:
- `feat: ...` - minor version bump
- `fix: ...` - patch version bump
- `feat!: ...` or `BREAKING CHANGE:` - major version bump

The firmware binary is attached to each GitHub release.
