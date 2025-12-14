#!/bin/bash

# Flash firmware to one or more ESP32-S3 devices
#
# Usage:
#   ./flash_devices.sh                    # Flash to auto-detected data ports
#   ./flash_devices.sh /dev/ttyACM0       # Flash to specific port
#   ./flash_devices.sh /dev/ttyACM0 /dev/ttyACM2  # Flash to multiple ports
#
# Note: If flashing fails, put devices in bootloader mode:
#   1. Hold BOOT button
#   2. Press and release RESET button
#   3. Release BOOT button
#   Then run this script again.

set -e

TARGET_BUILD="target/xtensa-esp32s3-none-elf/release/walkie-textie-rust-firmware"

# Source ESP toolchain (required for xtensa target)
if [ -f "$HOME/export-esp.sh" ]; then
    source "$HOME/export-esp.sh"
else
    echo "Error: ESP toolchain not found. Run 'espup install' first."
    exit 1
fi

# Auto-detect devices if none specified
if [ $# -eq 0 ]; then
    # Find all ttyACM devices and filter to even-numbered ones (data ports)
    # With dual CDC: ACM0=data1, ACM1=debug1, ACM2=data2, ACM3=debug2, etc.
    DEVICES=()
    for port in /dev/ttyACM*; do
        if [ -e "$port" ]; then
            # Extract number from port name
            num="${port##/dev/ttyACM}"
            # Only use even-numbered ports (data ports, not debug ports)
            if [ $((num % 2)) -eq 0 ]; then
                DEVICES+=("$port")
            fi
        fi
    done

    if [ ${#DEVICES[@]} -eq 0 ]; then
        echo "No devices found. Connect device(s) and try again."
        exit 1
    fi

    echo "Auto-detected data ports: ${DEVICES[*]}"
else
    DEVICES=("$@")
fi

# Check if ports are accessible
echo ""
echo "Checking port access..."
for PORT in "${DEVICES[@]}"; do
    if [ ! -e "$PORT" ]; then
        echo "Error: $PORT does not exist"
        exit 1
    fi
    if [ ! -w "$PORT" ]; then
        echo "Error: No write permission for $PORT"
        echo "Try: sudo usermod -a -G dialout \$USER (then log out and back in)"
        exit 1
    fi
done
echo "All ports accessible."

echo ""
echo "Building firmware..."
cargo +esp build --features embedded --release -Zbuild-std=core,alloc || { echo "Build failed!"; exit 1; }
echo "Firmware built successfully."

# Flash devices sequentially to avoid USB contention
echo ""
FAILED=()

for PORT in "${DEVICES[@]}"; do
    echo ""
    echo "Flashing $PORT..."
    if espflash flash --port "$PORT" "$TARGET_BUILD"; then
        echo "$PORT flashed successfully."
    else
        echo "Failed to flash $PORT"
        FAILED+=("$PORT")
    fi
done

echo ""
echo "=========================================="
if [ ${#FAILED[@]} -eq 0 ]; then
    echo "All devices flashed successfully!"
else
    echo "Flashing failed for: ${FAILED[*]}"
    echo ""
    echo "Troubleshooting:"
    echo "  1. Put device in bootloader mode:"
    echo "     - Hold BOOT button"
    echo "     - Press and release RESET"
    echo "     - Release BOOT button"
    echo "  2. Check USB cable supports data (not charge-only)"
    echo "  3. Try unplugging and replugging the device"
    exit 1
fi
