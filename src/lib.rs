#![cfg_attr(not(test), no_std)]

pub mod config;

// Wire protocol (command/response codec and COBS framing) shared with the app.
pub use wt_protocol;

// The lora module is always present so its dependency-free calibration helpers
// can be unit-tested on the host; the hardware driver/traits are gated inside it.
pub mod lora;

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
