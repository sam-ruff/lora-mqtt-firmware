#!/usr/bin/env bash
# Build the Rust engine as an xcframework for iOS (device + simulator).
#
# Requires macOS with Xcode. Run before `pod install`; the Podfile pulls the
# result in via ios/rust_backend.podspec and the xcconfigs force-load it.
set -euo pipefail

cd "$(dirname "$0")/../rust_backend"

rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios

# Static library only; the app links it, so a cdylib is never needed.
for target in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
  cargo rustc --lib --release --target "$target" --crate-type staticlib
done

# Combine the two simulator slices (Apple Silicon + Intel) into one static lib.
mkdir -p target/ios-sim/release
lipo -create \
  target/aarch64-apple-ios-sim/release/librust_backend.a \
  target/x86_64-apple-ios/release/librust_backend.a \
  -output target/ios-sim/release/librust_backend.a

OUT="../ios/Frameworks/rust_backend.xcframework"
rm -rf "$OUT"
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/librust_backend.a \
  -library target/ios-sim/release/librust_backend.a \
  -output "$OUT"

echo "Wrote $OUT"
