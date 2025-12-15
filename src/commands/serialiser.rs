//! Response serialiser with COBS encoding
//!
//! Serialises Response structs into COBS-encoded frames for transmission.

use crate::commands::parser::calculate_crc;
use crate::commands::types::Response;
use crate::config::protocol::{MAX_FRAME_SIZE, PROTOCOL_VERSION};
#[cfg(test)]
use crate::config::protocol::FRAME_DELIMITER;
use heapless::Vec;

/// Response IDs (mirrors command IDs for responses)
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum ResponseId {
    Version = 0x01,
    TxComplete = 0x10,
    RxPacket = 0x11,
    Error = 0xFF,
}

/// Serialiser for response frames
pub struct ResponseSerialiser;

impl ResponseSerialiser {
    /// Create a new response serialiser
    pub fn new() -> Self {
        Self
    }

    /// Serialise a response to a COBS-encoded frame
    ///
    /// Returns the complete frame including COBS encoding and zero delimiter.
    pub fn serialise(&self, response: &Response) -> Vec<u8, MAX_FRAME_SIZE> {
        // Build the raw frame first
        let raw = self.build_raw_frame(response);

        // COBS encode (corncobs::encode_buf includes the trailing zero delimiter)
        self.cobs_encode(&raw)
    }

    /// Build the raw (unencoded) frame with CRC
    ///
    /// Frame format: [version: u8][resp_id: u8][length: u16 LE][payload][crc16: u16 LE]
    fn build_raw_frame(&self, response: &Response) -> Vec<u8, MAX_FRAME_SIZE> {
        let mut frame: Vec<u8, MAX_FRAME_SIZE> = Vec::new();

        // Protocol version first
        let _ = frame.push(PROTOCOL_VERSION);

        match response {
            Response::Version {
                major,
                minor,
                patch,
            } => {
                let _ = frame.push(ResponseId::Version as u8);
                let _ = frame.extend_from_slice(&3u16.to_le_bytes()); // length = 3
                let _ = frame.push(*major);
                let _ = frame.push(*minor);
                let _ = frame.push(*patch);
            }
            Response::TxComplete => {
                let _ = frame.push(ResponseId::TxComplete as u8);
                let _ = frame.extend_from_slice(&0u16.to_le_bytes()); // length = 0
            }
            Response::RxPacket { data, rssi, snr } => {
                let _ = frame.push(ResponseId::RxPacket as u8);
                // Payload: data + rssi (2 bytes) + snr (1 byte)
                let payload_len = data.len() + 3;
                let _ = frame.extend_from_slice(&(payload_len as u16).to_le_bytes());
                let _ = frame.extend_from_slice(data);
                let _ = frame.extend_from_slice(&rssi.to_le_bytes());
                let _ = frame.push(*snr as u8);
            }
            Response::Error {
                status,
                original_command_id,
            } => {
                let _ = frame.push(ResponseId::Error as u8);
                let _ = frame.extend_from_slice(&2u16.to_le_bytes()); // length = 2
                let _ = frame.push(*status as u8);
                let _ = frame.push(*original_command_id);
            }
        }

        // Calculate and append CRC
        let crc = calculate_crc(&frame);
        let _ = frame.extend_from_slice(&crc.to_le_bytes());

        frame
    }

    /// COBS encode a buffer using corncobs
    fn cobs_encode(&self, data: &[u8]) -> Vec<u8, MAX_FRAME_SIZE> {
        let mut output: Vec<u8, MAX_FRAME_SIZE> = Vec::new();
        // Resize to max encoded length
        output.resize(corncobs::max_encoded_len(data.len()), 0).ok();
        let len = corncobs::encode_buf(data, &mut output);
        output.truncate(len);
        output
    }
}

impl Default for ResponseSerialiser {
    fn default() -> Self {
        Self::new()
    }
}

/// COBS decode using corncobs (for testing/verification)
#[allow(clippy::result_unit_err)]
pub fn cobs_decode(encoded: &[u8]) -> Result<Vec<u8, MAX_FRAME_SIZE>, ()> {
    let mut output: Vec<u8, MAX_FRAME_SIZE> = Vec::new();
    output.resize(encoded.len(), 0).map_err(|_| ())?;
    let len = corncobs::decode_buf(encoded, &mut output).map_err(|_| ())?;
    output.truncate(len);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::types::{CommandId, ResponseStatus};
    use crate::config::protocol::MAX_LORA_PAYLOAD;

    #[test]
    fn test_serialise_version() {
        use crate::config::protocol::PROTOCOL_VERSION;

        let serialiser = ResponseSerialiser::new();
        let response = Response::Version {
            major: 0,
            minor: 1,
            patch: 0,
        };

        let encoded = serialiser.serialise(&response);

        // Should end with delimiter
        assert_eq!(encoded[encoded.len() - 1], FRAME_DELIMITER);

        // Decode and verify (pass full encoded data including zero terminator)
        let decoded = cobs_decode(&encoded).expect("Should decode");

        // Check structure: [version][resp_id][length LE][major][minor][patch][crc LE]
        assert_eq!(decoded[0], PROTOCOL_VERSION); // protocol version
        assert_eq!(decoded[1], ResponseId::Version as u8);
        assert_eq!(decoded[2], 3); // length low byte
        assert_eq!(decoded[3], 0); // length high byte
        assert_eq!(decoded[4], 0); // major
        assert_eq!(decoded[5], 1); // minor
        assert_eq!(decoded[6], 0); // patch
    }

    #[test]
    fn test_serialise_tx_complete() {
        let serialiser = ResponseSerialiser::new();
        let response = Response::TxComplete;

        let encoded = serialiser.serialise(&response);
        assert_eq!(encoded[encoded.len() - 1], FRAME_DELIMITER);
    }

    #[test]
    fn test_serialise_rx_packet() {
        use crate::config::protocol::PROTOCOL_VERSION;

        let serialiser = ResponseSerialiser::new();
        let mut data: Vec<u8, MAX_LORA_PAYLOAD> = Vec::new();
        data.extend_from_slice(&[0x48, 0x65, 0x6C, 0x6C, 0x6F])
            .unwrap(); // "Hello"

        let response = Response::RxPacket {
            data,
            rssi: -50,
            snr: 10,
        };

        let encoded = serialiser.serialise(&response);
        assert_eq!(encoded[encoded.len() - 1], FRAME_DELIMITER);

        // Decode and verify structure (pass full encoded data including zero terminator)
        let decoded = cobs_decode(&encoded).expect("Should decode");

        // Check structure: [version][resp_id][length LE][payload][crc LE]
        assert_eq!(decoded[0], PROTOCOL_VERSION);
        assert_eq!(decoded[1], ResponseId::RxPacket as u8);
        // Length should be 5 (data) + 2 (rssi) + 1 (snr) = 8
        let length = u16::from_le_bytes([decoded[2], decoded[3]]);
        assert_eq!(length, 8);
    }

    #[test]
    fn test_serialise_error() {
        use crate::config::protocol::PROTOCOL_VERSION;

        let serialiser = ResponseSerialiser::new();
        let response = Response::Error {
            status: ResponseStatus::Timeout,
            original_command_id: CommandId::LoraTx as u8,
        };

        let encoded = serialiser.serialise(&response);
        assert_eq!(encoded[encoded.len() - 1], FRAME_DELIMITER);

        // Decode and verify (pass full encoded data including zero terminator)
        let decoded = cobs_decode(&encoded).expect("Should decode");

        // Check structure: [version][resp_id][length LE][status][cmd_id][crc LE]
        assert_eq!(decoded[0], PROTOCOL_VERSION);
        assert_eq!(decoded[1], ResponseId::Error as u8);
        assert_eq!(decoded[4], ResponseStatus::Timeout as u8);
        assert_eq!(decoded[5], CommandId::LoraTx as u8);
    }

    #[test]
    fn test_cobs_roundtrip() {
        let serialiser = ResponseSerialiser::new();

        // Test data with zeros
        let data_with_zeros = [0x01, 0x00, 0x02, 0x00, 0x03];
        let encoded = serialiser.cobs_encode(&data_with_zeros);

        println!("Encoded: {:02x?}", encoded.as_slice());

        // Should end with zero delimiter
        assert_eq!(encoded[encoded.len() - 1], 0x00, "Should end with zero");

        // Encoded data (except delimiter) should have no zeros
        for &byte in &encoded[..encoded.len() - 1] {
            assert_ne!(byte, 0, "COBS encoded data should not contain zeros");
        }

        // Decode WITH the delimiter (corncobs expects it)
        let decoded = cobs_decode(&encoded).expect("Should decode");
        println!("Decoded: {:02x?}", decoded.as_slice());
        assert_eq!(decoded.as_slice(), &data_with_zeros);
    }
}
