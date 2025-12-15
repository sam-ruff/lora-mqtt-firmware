//! Command and response types for the binary protocol
//!
//! Protocol format (before COBS encoding):
//! ```text
//! [version: u8][command_id: u8][length: u16 LE][payload: [u8; length]][crc16: u16 LE]
//! ```

use crate::config::protocol::MAX_LORA_PAYLOAD;
use heapless::Vec;

/// Command IDs for the binary protocol
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    /// Get firmware version
    GetVersion = 0x01,
    /// Normal reboot (restart firmware)
    Reboot = 0x03,
    /// Transmit LoRa packet
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
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseStatus {
    /// Command executed successfully
    Success = 0x00,
    /// Unknown or invalid command ID
    InvalidCommand = 0x01,
    /// Invalid payload length
    InvalidLength = 0x02,
    /// CRC mismatch
    CrcError = 0x03,
    /// Unsupported protocol version
    InvalidVersion = 0x04,
    /// LoRa radio error
    LoraError = 0x10,
    /// Operation timed out
    Timeout = 0x11,
}

/// Response to a command
#[derive(Debug, Clone)]
pub enum Response {
    /// Version response
    Version { major: u8, minor: u8, patch: u8 },

    /// Transmit complete acknowledgement
    TxComplete,

    /// Received LoRa packet
    RxPacket {
        data: Vec<u8, MAX_LORA_PAYLOAD>,
        rssi: i16,
        snr: i8,
    },

    /// Error response
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
