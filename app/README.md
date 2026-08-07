# Walkie-Textie Hub configurator

Phone app for setting up a Walkie-Textie hub: join the hub's provisioning
hotspot, connect, and configure WiFi, MQTT and the LoRaWAN gateway. It drives
the same JSON API as the hub's captive portal.

The HTTP client and validation live in a Rust engine (`rust_backend/`)
exposed through flutter_rust_bridge, matching the Walkie-Textie messenger
app's architecture.

```bash
./build_rust.sh                        # FRB codegen + build + test the engine
flutter run --dart-define=MOCK=true    # full flow against a fake hub
flutter run                            # against a real hub
```

Android cross-compiles the engine automatically from Gradle via cargo-ndk.
For iOS, run `tool/build_ios_libs.sh` on a Mac before `pod install`; the
xcconfigs force-load the resulting static library into the app binary.

Note for cargo invocations inside `rust_backend/`: the surrounding firmware
repo defaults cargo to the xtensa target, so set
`CARGO_BUILD_TARGET=$(rustc -vV | awk '/^host/ {print $2}')` (build_rust.sh
does this) or pass an explicit `--target`.
