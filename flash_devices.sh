#!/bin/bash

TARGET_BUILD="target/xtensa-esp32s3-none-elf/release/walkie-textie-rust-firmware"

if [ $# -eq 0 ]; then
    DEVICES=("/dev/ttyACM0" "/dev/ttyACM1")
    echo "No devices specified, using defaults: ${DEVICES[*]}"
else
    DEVICES=("$@")
fi

echo "Building firmware..."
cargo +esp build --target xtensa-esp32s3-none-elf --features embedded --release -Zbuild-std=core,alloc || { echo "Build failed!"; exit 1; }
echo "Firmware built successfully."

PIDS=()

for PORT in "${DEVICES[@]}"; do
    echo "Starting flash for $PORT in background..."
    espflash flash --port "$PORT" "$TARGET_BUILD" &
    PIDS+=($!)
done

echo "Waiting for flashing operations to complete..."
for PID in "${PIDS[@]}"; do
    wait "$PID"
done

echo "All flashing operations completed."