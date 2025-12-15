//! Command and response types for the binary protocol
//!
//! # Protocol Format
//!
//! All frames use COBS encoding with a zero byte delimiter:
//! ```text
//! [COBS-encoded payload][0x00]
//! ```
//!
//! The payload format (before COBS encoding):
//! ```text
//! [version: u8][cmd_id: u8][length: u16 LE][payload: [u8; length]][crc16: u16 LE]
//! ```
//!
//! - `version`: Protocol version (currently 1)
//! - `cmd_id`: Command or response identifier
//! - `length`: Payload length in bytes (little-endian)
//! - `payload`: Variable-length data (0-256 bytes)
//! - `crc16`: CRC-16-XMODEM checksum over all preceding bytes
//!
//! # CRC Calculation
//!
//! Uses CRC-16-XMODEM (polynomial 0x1021, init 0x0000) over:
//! `[version][cmd_id][length_lo][length_hi][payload...]`

use crate::config::protocol::MAX_LORA_PAYLOAD;
use heapless::Vec;

/// Command IDs for the binary protocol
///
/// Commands are sent from the host to the device. Each command has a specific
/// payload format and expected response.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    /// Get firmware version (0x01)
    ///
    /// - Payload: None (length = 0)
    /// - Response: [`Response::Version`]
    GetVersion = 0x01,

    /// Reboot the device (0x03)
    ///
    /// - Payload: None (length = 0)
    /// - Response: None (device reboots immediately)
    Reboot = 0x03,

    /// Transmit data over LoRa (0x10)
    ///
    /// - Payload: Data bytes (1-256 bytes)
    /// - Response: [`Response::TxComplete`] on success
    LoraTx = 0x10,
}

impl CommandId {
    /// Try to convert a byte to a CommandId
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x01 => Some(Self::GetVersion),
            0x03 => Some(Self::Reboot),
            0x10 => Some(Self::LoraTx),
            _ => None,
        }
    }
}

/// Parsed command with associated data
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Get firmware version
    GetVersion,

    /// Normal reboot (restart firmware)
    Reboot,

    /// Transmit LoRa packet
    LoraTx { data: Vec<u8, MAX_LORA_PAYLOAD> },
}

impl Command {
    /// Get the command ID for this command
    pub fn id(&self) -> CommandId {
        match self {
            Command::GetVersion => CommandId::GetVersion,
            Command::Reboot => CommandId::Reboot,
            Command::LoraTx { .. } => CommandId::LoraTx,
        }
    }
}

/// Response status codes
///
/// Used in [`Response::Error`] to indicate why a command failed.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseStatus {
    /// Command executed successfully (0x00)
    Success = 0x00,

    /// Unknown or invalid command ID (0x01)
    InvalidCommand = 0x01,

    /// Payload length invalid for the command (0x02)
    ///
    /// Examples: non-zero payload for GetVersion, empty payload for LoraTx
    InvalidLength = 0x02,

    /// CRC-16 checksum mismatch (0x03)
    CrcError = 0x03,

    /// Protocol version not supported (0x04)
    ///
    /// The device only accepts commands with matching protocol version.
    InvalidVersion = 0x04,

    /// LoRa radio error during operation (0x10)
    LoraError = 0x10,

    /// Operation timed out (0x11)
    Timeout = 0x11,
}

/// Response to a command
///
/// Responses are sent from the device to the host. They use the same frame
/// format as commands but with response IDs instead of command IDs.
///
/// # Response IDs
///
/// | ID   | Response   | Description                    |
/// |------|------------|--------------------------------|
/// | 0x01 | Version    | Firmware version               |
/// | 0x10 | TxComplete | LoRa TX completed              |
/// | 0x11 | RxPacket   | Received LoRa packet           |
/// | 0xFF | Error      | Error with status code         |
#[derive(Debug, Clone)]
pub enum Response {
    /// Version response (ID: 0x01)
    ///
    /// Payload: `[major: u8][minor: u8][patch: u8]`
    Version { major: u8, minor: u8, patch: u8 },

    /// Transmit complete acknowledgement (ID: 0x10)
    ///
    /// Payload: None (length = 0)
    TxComplete,

    /// Received LoRa packet (ID: 0x11) - Unsolicited
    ///
    /// Sent automatically when a LoRa packet is received.
    ///
    /// Payload: `[data...][rssi: i16 LE][snr: i8]`
    /// - `data`: Received bytes (variable length)
    /// - `rssi`: Received signal strength in dBm
    /// - `snr`: Signal-to-noise ratio in dB
    RxPacket {
        data: Vec<u8, MAX_LORA_PAYLOAD>,
        rssi: i16,
        snr: i8,
    },

    /// Error response (ID: 0xFF)
    ///
    /// Payload: `[status: u8][original_command_id: u8]`
    Error {
        status: ResponseStatus,
        original_command_id: u8,
    },
}

impl Response {
    /// Create an error response for a given command
    pub fn error(status: ResponseStatus, command_id: CommandId) -> Self {
        Self::Error {
            status,
            original_command_id: command_id as u8,
        }
    }

    /// Create an error response with raw command ID (for unknown commands)
    pub fn error_raw(status: ResponseStatus, original_command_id: u8) -> Self {
        Self::Error {
            status,
            original_command_id,
        }
    }
}
