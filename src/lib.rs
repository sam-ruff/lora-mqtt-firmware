#![cfg_attr(not(test), no_std)]

pub mod commands;
pub mod config;
pub mod protocol;

// These modules depend on embassy/async features only available with embedded feature
#[cfg(feature = "embedded")]
pub mod ble;
#[cfg(feature = "embedded")]
pub mod debug;
#[cfg(feature = "embedded")]
pub mod dispatcher;
#[cfg(feature = "embedded")]
pub mod lora;

/// No-op debug macro for non-embedded builds (tests).
/// The real implementation is in src/debug.rs for embedded builds.
#[cfg(not(feature = "embedded"))]
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        // No-op in non-embedded builds
    };
}
