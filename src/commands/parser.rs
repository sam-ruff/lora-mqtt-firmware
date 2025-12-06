//! Command parser for COBS-decoded frames
//!
//! Parses binary protocol frames into Command structs.

use crate::commands::types::{Command, CommandId, ResponseStatus};
use crate::config::protocol::{MAX_LORA_PAYLOAD, PROTOCOL_VERSION};
use crc::{Crc, CRC_16_XMODEM};
use heapless::Vec;

const CRC: Crc<u16> = Crc::<u16>::new(&CRC_16_XMODEM);

/// Parser for binary protocol commands
pub struct CommandParser;

impl CommandParser {
    /// Create a new command parser
    pub fn new() -> Self {
        Self
    }

    /// Parse a COBS-decoded frame into a command
    ///
    /// Frame format: [version: u8][cmd_id: u8][length: u16 LE][payload][crc16: u16 LE]
    /// Minimum frame size: 1 (ver) + 1 (cmd) + 2 (length) + 0 (payload) + 2 (crc) = 6 bytes
    pub fn parse(&self, data: &[u8]) -> Result<Command, ResponseStatus> {
        // Minimum frame size check
        if data.len() < 6 {
            return Err(ResponseStatus::InvalidLength);
        }

        let version = data[0];
        let command_id_byte = data[1];
        let length = u16::from_le_bytes([data[2], data[3]]) as usize;

        // Check protocol version
        if version != PROTOCOL_VERSION {
            return Err(ResponseStatus::InvalidVersion);
        }

        // Check if we have enough bytes for payload + CRC
        let expected_len = 4 + length + 2;
        if data.len() < expected_len {
            return Err(ResponseStatus::InvalidLength);
        }

        let payload = &data[4..4 + length];
        let received_crc = u16::from_le_bytes([data[4 + length], data[5 + length]]);

        // Verify CRC over version + command_id + length + payload
        let calculated_crc = Self::calculate_crc(&data[..4 + length]);
        if calculated_crc != received_crc {
            return Err(ResponseStatus::CrcError);
        }

        // Parse based on command ID
        let command_id = CommandId::from_byte(command_id_byte);
        match command_id {
            Some(CommandId::GetVersion) => {
                if length != 0 {
                    return Err(ResponseStatus::InvalidLength);
                }
                Ok(Command::GetVersion)
            }
            Some(CommandId::LoraTx) => {
                if length == 0 || length > MAX_LORA_PAYLOAD {
                    return Err(ResponseStatus::InvalidLength);
                }
                let mut data_vec = Vec::new();
                data_vec
                    .extend_from_slice(payload)
                    .map_err(|_| ResponseStatus::InvalidLength)?;
                Ok(Command::LoraTx { data: data_vec })
            }
            None => Err(ResponseStatus::InvalidCommand),
        }
    }

    /// Calculate CRC-16-XMODEM
    fn calculate_crc(data: &[u8]) -> u16 {
        CRC.checksum(data)
    }
}

impl Default for CommandParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate CRC-16-CCITT for external use (e.g., building test frames)
pub fn calculate_crc(data: &[u8]) -> u16 {
    CommandParser::calculate_crc(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_frame(cmd_id: u8, payload: &[u8]) -> Vec<u8, 512> {
        let mut frame = Vec::new();
        frame.push(PROTOCOL_VERSION).unwrap(); // protocol version
        frame.push(cmd_id).unwrap();
        frame
            .extend_from_slice(&(payload.len() as u16).to_le_bytes())
            .unwrap();
        frame.extend_from_slice(payload).unwrap();

        let crc = calculate_crc(&frame);
        frame.extend_from_slice(&crc.to_le_bytes()).unwrap();
        frame
    }

    #[test]
    fn test_parse_get_version() {
        let parser = CommandParser::new();
        let frame = build_frame(0x01, &[]);

        let cmd = parser.parse(&frame).expect("Should parse");
        assert!(matches!(cmd, Command::GetVersion));
    }

    #[test]
    fn test_parse_lora_tx() {
        let parser = CommandParser::new();
        let payload = [0x48, 0x65, 0x6C, 0x6C, 0x6F]; // "Hello"
        let frame = build_frame(0x10, &payload);

        let cmd = parser.parse(&frame).expect("Should parse");
        match cmd {
            Command::LoraTx { data } => {
                assert_eq!(data.as_slice(), &payload);
            }
            _ => panic!("Expected LoraTx"),
        }
    }

    #[test]
    fn test_invalid_crc() {
        let parser = CommandParser::new();
        let mut frame = build_frame(0x01, &[]);
        // Corrupt the CRC
        let len = frame.len();
        frame[len - 1] ^= 0xFF;

        let result = parser.parse(&frame);
        assert_eq!(result, Err(ResponseStatus::CrcError));
    }

    #[test]
    fn test_invalid_command() {
        let parser = CommandParser::new();
        let frame = build_frame(0xFE, &[]); // 0xFE is invalid (0xFF is Error response)

        let result = parser.parse(&frame);
        assert_eq!(result, Err(ResponseStatus::InvalidCommand));
    }

    #[test]
    fn test_invalid_version() {
        let parser = CommandParser::new();
        // Build frame with wrong version
        let mut frame: Vec<u8, 512> = Vec::new();
        frame.push(0x00).unwrap(); // wrong version (0 instead of 1)
        frame.push(0x01).unwrap(); // GetVersion command
        frame.extend_from_slice(&0u16.to_le_bytes()).unwrap();
        let crc = calculate_crc(&frame);
        frame.extend_from_slice(&crc.to_le_bytes()).unwrap();

        let result = parser.parse(&frame);
        assert_eq!(result, Err(ResponseStatus::InvalidVersion));
    }

    #[test]
    fn test_too_short() {
        let parser = CommandParser::new();
        let result = parser.parse(&[0x01, 0x01, 0x00]);
        assert_eq!(result, Err(ResponseStatus::InvalidLength));
    }
}
