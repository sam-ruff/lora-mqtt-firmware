#!/bin/bash

TARGET_BUILD="target/xtensa-esp32s3-none-elf/release/walkie-textie-rust-firmware"

echo "Building firmware..."
cargo +esp build --target xtensa-esp32s3-none-elf --features embedded --release -Zbuild-std=core,alloc || { echo "Build failed!"; exit 1; }
echo "Firmware built successfully."

PORT1="/dev/ttyACM0"
PORT2="/dev/ttyACM1"

echo "Starting flash for $PORT1 in background..."
espflash flash --port "$PORT1" "$TARGET_BUILD" &
PID1=$! # Get the Process ID of the last background command

echo "Starting flash for $PORT2 in background..."
espflash flash --port "$PORT2" "$TARGET_BUILD" &
PID2=$! # Get the Process ID of the last background command

echo "Waiting for flashing operations to complete..."
wait "$PID1" # Wait for the first espflash process to finish
wait "$PID2" # Wait for the second espflash process to finish

echo "All flashing operations completed."