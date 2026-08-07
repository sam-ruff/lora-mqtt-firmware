#![cfg_attr(not(test), no_std)]

pub mod config;

// Wire protocol (command/response codec and COBS framing) shared with the app.
pub use wt_protocol;

// The lora module is always present so its dependency-free calibration helpers
// can be unit-tested on the host; the hardware driver/traits are gated inside it.
pub mod lora;

// Hub configuration (defaults, validation, command application) is pure and
// host-tested; only the flash store inside it is embedded-gated.
pub mod hub_config;

// Network-side helpers (stats atomics, backoff, MQTT seam) are host-tested.
pub mod net;

// The LoRa <-> MQTT bridge state machine is pure and host-tested; only its
// embassy channels are embedded-gated.
pub mod bridge;

// The LoRaWAN gateway core (Semtech UDP codec, forwarder policy, downlink
// scheduler) is pure and host-tested; its embassy channels are embedded-gated
// and the DownlinkJob type needs the radio config, so the module follows the
// lora module's gating.
#[cfg(any(feature = "embedded", feature = "host-test"))]
pub mod gateway;

// The provisioning portal codecs (DHCP, DNS, HTTP routing and payloads) are
// pure and host-tested; the socket tasks live under tasks/ in the binary.
pub mod portal;

// These modules depend on embassy/async features only available with embedded feature
#[cfg(feature = "embedded")]
pub mod debug;
// The dispatcher itself is plain async and unit-tested on the host; only its
// embassy-sync channels are embedded-gated (inside the module).
#[cfg(any(feature = "embedded", feature = "host-test"))]
pub mod dispatcher;

/// No-op debug macro for non-embedded builds (tests).
/// The real implementation is in src/debug.rs for embedded builds.
#[cfg(not(feature = "embedded"))]
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        // No-op in non-embedded builds
    };
}
