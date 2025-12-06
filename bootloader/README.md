# Silent Bootloader for ESP32-S3

This directory contains a minimal ESP-IDF project to build a bootloader with logging disabled.

## What the Bootloader Does

The ESP32-S3 uses a two-stage boot process:

1. **First Stage (ROM Bootloader)**: Built into the chip's ROM, this runs immediately on power-on. It initialises basic hardware, checks strapping pins to determine boot mode, and loads the second stage bootloader from flash at address 0x0.

2. **Second Stage (ESP-IDF Bootloader)**: This is what we build here. It performs:
   - Initialises flash and memory mappings
   - Reads the partition table from flash (at 0x8000)
   - Selects the correct app partition to boot (supports OTA updates)
   - Validates the app image (checks header, segments, and optionally secure boot signatures)
   - Loads app segments into RAM and sets up the memory map
   - Jumps to the application entry point

## Project Structure

```
bootloader/
├── CMakeLists.txt        # ESP-IDF project file
├── sdkconfig.defaults    # Build configuration (silent logging)
├── main/
│   ├── CMakeLists.txt    # Component registration
│   └── main.c            # Minimal app (required by ESP-IDF build system)
└── README.md
```

The `main.c` file is a dummy application required by the ESP-IDF build system. We only care about the bootloader binary it produces, not the application.

## Prerequisites

Install ESP-IDF v5.2 or later:

```bash
mkdir -p ~/esp
cd ~/esp
git clone -b v5.2.2 --recursive https://github.com/espressif/esp-idf.git
cd esp-idf
./install.sh esp32s3
```

## Building

```bash
# Source ESP-IDF environment
source ~/esp/esp-idf/export.sh

# Navigate to this directory
cd bootloader

# Set target and build
idf.py set-target esp32s3
idf.py build
```

The bootloader binary will be at `build/bootloader/bootloader.bin`.

## Flashing

Copy the bootloader to the project root and flash with the firmware:

```bash
cp build/bootloader/bootloader.bin ../silent-bootloader-esp32s3.bin

espflash flash --port /dev/ttyACM0 \
    --bootloader ../silent-bootloader-esp32s3.bin \
    ../target/xtensa-esp32s3-none-elf/release/walkie-textie-rust-firmware
```

## Configuration

The `sdkconfig.defaults` file sets:
- `CONFIG_BOOTLOADER_LOG_LEVEL_NONE=y` - Disables all bootloader log output
