//! Configuration constants for the ESP32-S3 with WIO-SX1262

/// TCXO configuration
pub mod tcxo {
    /// TCXO voltage code for SX1262 register
    /// 0x02 = 1.8V
    pub const VOLTAGE_CODE: u8 = 0x02;
}

/// Default LoRa configuration
pub mod lora_defaults {
    /// Frequency in Hz (869.525 MHz)
    pub const FREQUENCY_HZ: u32 = 869_525_000;
    pub const SPREADING_FACTOR: u8 = 11;
    pub const BANDWIDTH_KHZ: u32 = 250;
    /// Coding rate 4/8 (higher redundancy)
    pub const CODING_RATE: u8 = 8;
    /// TX power in dBm (supports -9 to +22)
    pub const TX_POWER_DBM: i8 = 22;
}

/// Protocol constants
pub mod protocol {
    /// Frame delimiter for COBS encoding
    pub const FRAME_DELIMITER: u8 = 0x00;

    /// Maximum frame size
    pub const MAX_FRAME_SIZE: usize = 512;

    /// Maximum payload size for LoRa
    pub const MAX_LORA_PAYLOAD: usize = 256;

    /// Protocol version (increment when message format changes)
    pub const PROTOCOL_VERSION: u8 = 1;

    /// Firmware version
    pub const VERSION_MAJOR: u8 = 0;
    pub const VERSION_MINOR: u8 = 1;
    pub const VERSION_PATCH: u8 = 0;
}
