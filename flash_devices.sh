#!/bin/bash

# Flash firmware to one or more ESP32-S3 devices
#
# Usage:
#   ./flash_devices.sh                           # Flash all detected ttyACM ports
#   ./flash_devices.sh /dev/ttyACM0              # Flash to specific port
#   ./flash_devices.sh /dev/ttyACM0 /dev/ttyACM1 # Flash to multiple ports in parallel
#
# Note: Devices must be in bootloader mode before flashing:
#   1. Hold BOOT button
#   2. Press and release RESET button
#   3. Release BOOT button
#   Then run this script.

set -e

TARGET_BUILD="target/xtensa-esp32s3-none-elf/release/walkie-textie-rust-firmware"

# Parse ports from arguments
PORTS=("$@")

# Source ESP toolchain (required for xtensa target)
if [ -f "$HOME/export-esp.sh" ]; then
    source "$HOME/export-esp.sh"
else
    echo "Error: ESP toolchain not found. Run 'espup install' first."
    exit 1
fi

# Auto-detect devices if none specified
if [ ${#PORTS[@]} -eq 0 ]; then
    # Find all ttyACM devices (in bootloader mode, each device has one port)
    for port in /dev/ttyACM*; do
        if [ -e "$port" ]; then
            PORTS+=("$port")
        fi
    done

    if [ ${#PORTS[@]} -eq 0 ]; then
        echo "No devices found. Connect device(s) and try again."
        exit 1
    fi

    echo "Auto-detected ports: ${PORTS[*]}"
fi

DEVICES=("${PORTS[@]}")

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

# Flash devices in parallel with coloured output
echo ""
echo "Flashing ${#DEVICES[@]} device(s) in parallel..."

# Colours for different devices
COLOURS=('\033[0;32m' '\033[0;34m' '\033[0;35m' '\033[0;36m')  # green, blue, magenta, cyan
RESET='\033[0m'

# Create temp directory for status files
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

# Launch flash processes in parallel with coloured output
PIDS=()
for i in "${!DEVICES[@]}"; do
    PORT="${DEVICES[$i]}"
    COLOUR="${COLOURS[$((i % ${#COLOURS[@]}))]}"
    PORT_NAME=$(basename $PORT)
    (
        set -o pipefail
        espflash flash --no-skip --port "$PORT" "$TARGET_BUILD" 2>&1 | while IFS= read -r line; do
            echo -e "${COLOUR}[${PORT_NAME}]${RESET} $line"
        done
        if [ $? -eq 0 ]; then
            echo "success" > "$TEMP_DIR/$PORT_NAME.status"
        else
            echo "failed" > "$TEMP_DIR/$PORT_NAME.status"
        fi
    ) &
    PIDS+=($!)
done

# Wait for all flash processes to complete
for PID in "${PIDS[@]}"; do
    wait $PID 2>/dev/null || true
done

# Collect results
echo ""
FAILED=()
for PORT in "${DEVICES[@]}"; do
    PORT_NAME=$(basename $PORT)
    if [ -f "$TEMP_DIR/$PORT_NAME.status" ] && [ "$(cat $TEMP_DIR/$PORT_NAME.status)" = "success" ]; then
        echo -e "\033[0;32m$PORT flashed successfully.\033[0m"
    else
        echo -e "\033[0;31m$PORT FAILED.\033[0m"
        FAILED+=("$PORT")
    fi
done

echo ""
sleep 2
# Reset all devices to boot into new firmware (in case still in bootloader mode)
echo "Resetting devices..."
for PORT in "${DEVICES[@]}"; do
    espflash reset --port "$PORT" 2>/dev/null &
done
wait
echo "Devices reset complete."

echo ""
echo "=========================================="
if [ ${#FAILED[@]} -eq 0 ]; then
    echo "All devices flashed successfully!"
else
    echo "Flashing failed for: ${FAILED[*]}"
    echo ""
    echo "Troubleshooting:"
    echo "  1. Put device in bootloader mode manually:"
    echo "     - Hold BOOT button"
    echo "     - Press and release RESET"
    echo "     - Release BOOT button"
    echo "  2. Check USB cable supports data (not charge-only)"
    echo "  3. Try unplugging and replugging the device"
    exit 1
fi
