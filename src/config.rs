//! Configuration constants for the ESP32-S3 with WIO-SX1262

/// TCXO configuration
pub mod tcxo {
    /// TCXO voltage code for SX1262 register
    /// 0x02 = 1.8V
    pub const VOLTAGE_CODE: u8 = 0x02;
}

/// Default LoRa configuration
pub mod lora_defaults {
    /// Frequency in Hz (869.525 MHz), in the EU868 869.4-869.65 MHz sub-band
    /// (10% duty cycle, 500 mW ERP).
    pub const FREQUENCY_HZ: u32 = 869_525_000;
    /// Long-range default. Adjustable at runtime (7-12) via the
    /// SetSpreadingFactor host command; resets to this default on reboot.
    pub const SPREADING_FACTOR: u8 = 11;
    pub const BANDWIDTH_KHZ: u32 = 250;
    /// Coding rate 4/8 (higher redundancy)
    pub const CODING_RATE: u8 = 8;
    /// TX power in dBm (supports -9 to +22)
    pub const TX_POWER_DBM: i8 = 22;
}

/// Protocol constants
pub mod protocol {
    /// Wire size limits are owned by the shared `wt-protocol` crate so the
    /// firmware and app cannot drift.
    pub use wt_protocol::{MAX_FRAME_SIZE, MAX_LORA_PAYLOAD};

    /// Firmware version, reported by GetVersion.
    pub const VERSION_MAJOR: u8 = 0;
    pub const VERSION_MINOR: u8 = 1;
    pub const VERSION_PATCH: u8 = 0;
}
