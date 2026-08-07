#!/bin/bash
# Generate FRB bindings and build the Rust engine for the host.
#
# The firmware repo's .cargo/config.toml defaults every cargo invocation to
# the xtensa target; pin the host triple so the engine (and codegen's cargo
# expand) build natively. Mobile builds override the target themselves
# (cargo-ndk on Android, explicit --target in tool/build_ios_libs.sh).

set -e
cd "$(dirname "$0")"

HOST_TRIPLE=$(rustc -vV | awk '/^host/ {print $2}')
export CARGO_BUILD_TARGET="$HOST_TRIPLE"

echo "Step 1: Generate FRB bindings..."
flutter_rust_bridge_codegen generate

echo "Step 2: Build and test the Rust engine..."
cd rust_backend
cargo build --release
cargo test

# flutter_rust_bridge's default loader looks in target/release, but the
# pinned triple puts artifacts under target/<triple>/release; mirror the
# shared library for desktop dev runs.
mkdir -p target/release
for lib in librust_backend.so librust_backend.dylib; do
    if [ -f "target/$HOST_TRIPLE/release/$lib" ]; then
        cp "target/$HOST_TRIPLE/release/$lib" target/release/
    fi
done
cd ..

echo "Done. Run the app with: flutter run --dart-define=MOCK=true"
