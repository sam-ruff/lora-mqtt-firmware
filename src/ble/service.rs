//! Nordic UART Service (NUS) definition
//!
//! Implements the standard Nordic UART Service for BLE serial communication.
//! - Service UUID: 6E400001-B5A3-F393-E0A9-E50E24DCCA9E
//! - RX Characteristic: 6E400002-... (write, write without response)
//! - TX Characteristic: 6E400003-... (notify)

use trouble_host::prelude::*;

/// Maximum BLE packet size for NUS
/// Using a fixed-size array that fits within GATT constraints
pub const NUS_MAX_PACKET_SIZE: usize = 128;

/// Nordic UART Service
///
/// This service provides a simple way to transfer data over BLE using
/// a UART-like interface with RX (receive) and TX (transmit) characteristics.
#[gatt_service(uuid = "6e400001-b5a3-f393-e0a9-e50e24dcca9e")]
pub struct NordicUartService {
    /// RX Characteristic - client writes COBS frames here
    #[characteristic(uuid = "6e400002-b5a3-f393-e0a9-e50e24dcca9e", write, write_without_response, value = [0u8; 128])]
    pub rx: [u8; NUS_MAX_PACKET_SIZE],

    /// TX Characteristic - server notifies COBS frames here
    #[characteristic(uuid = "6e400003-b5a3-f393-e0a9-e50e24dcca9e", notify, value = [0u8; 128])]
    pub tx: [u8; NUS_MAX_PACKET_SIZE],
}
