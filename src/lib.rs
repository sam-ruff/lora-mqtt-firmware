#![cfg_attr(not(test), no_std)]

pub mod commands;
pub mod config;
pub mod protocol;

// These modules depend on embassy/async features only available with embedded feature
#[cfg(feature = "embedded")]
pub mod ble;
#[cfg(feature = "embedded")]
pub mod dispatcher;
#[cfg(feature = "embedded")]
pub mod lora;
#[cfg(feature = "embedded")]
pub mod serial;
