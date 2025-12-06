//! Hardware configuration constants for the ESP32-S3 with WIO-SX1262

/// LED pin
pub mod led {
    pub const PIN: u8 = 48;
}

/// SPI pins for LoRa module
pub mod spi {
    pub const SCLK: u8 = 7;
    pub const MISO: u8 = 8;
    pub const MOSI: u8 = 9;
}

/// LoRa control pins
pub mod lora_pins {
    pub const NSS: u8 = 41;
    pub const DIO1: u8 = 39;
    pub const NRST: u8 = 42;
    pub const BUSY: u8 = 40;
    pub const DIO2: u8 = 38;
}

/// TCXO configuration
pub mod tcxo {
    /// TCXO voltage in volts (1.8V for WIO-SX1262)
    pub const VOLTAGE_V: f32 = 1.8;

    /// TCXO voltage code for SX1262 register
    /// 0x02 = 1.8V
    pub const VOLTAGE_CODE: u8 = 0x02;
}

/// Default LoRa configuration
pub mod lora_defaults {
    /// EU ISM band frequency
    pub const FREQUENCY_HZ: u32 = 868_000_000;
    pub const SPREADING_FACTOR: u8 = 7;
    pub const BANDWIDTH_KHZ: u32 = 125;
    /// Coding rate 4/5
    pub const CODING_RATE: u8 = 5;
    pub const TX_POWER_DBM: i8 = 14;
}

/// Serial configuration
pub mod serial {
    pub const BAUD_RATE: u32 = 115200;
    pub const RX_BUFFER_SIZE: usize = 512;
    pub const TX_BUFFER_SIZE: usize = 512;
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
