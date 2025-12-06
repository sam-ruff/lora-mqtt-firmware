//! Serial command reader
//!
//! Reads bytes from a serial port, accumulates COBS frames,
//! decodes them, and parses commands.

use crate::commands::parser::CommandParser;
use crate::commands::serialiser::cobs_decode;
use crate::commands::types::{Command, ResponseStatus};
use crate::config::protocol::MAX_FRAME_SIZE;
use crate::protocol::framing::FrameAccumulator;
use crate::serial::traits::{SerialError, SerialPort};

/// Result of attempting to read a command
#[derive(Debug)]
pub enum ReadResult {
    /// Successfully parsed a command
    Command(Command),
    /// Parse error (should send error response)
    ParseError(ResponseStatus, u8),
    /// Serial error
    SerialError(SerialError),
}

/// Serial command reader
///
/// Wraps a serial port and provides command parsing functionality.
/// Handles COBS framing and protocol parsing.
pub struct SerialCommandReader {
    accumulator: FrameAccumulator,
    parser: CommandParser,
    sequence_counter: u16,
}

impl SerialCommandReader {
    /// Create a new serial command reader
    pub fn new() -> Self {
        Self {
            accumulator: FrameAccumulator::new(),
            parser: CommandParser::new(),
            sequence_counter: 0,
        }
    }

    /// Get the next sequence ID
    pub fn next_sequence_id(&mut self) -> u16 {
        let id = self.sequence_counter;
        self.sequence_counter = self.sequence_counter.wrapping_add(1);
        id
    }

    /// Read and parse a complete command from the serial port
    ///
    /// This method blocks until a complete valid command is received
    /// or an error occurs.
    pub async fn read_command<S: SerialPort>(&mut self, serial: &mut S) -> ReadResult {
        let mut read_buf = [0u8; 64];

        loop {
            let bytes_read = match serial.read(&mut read_buf).await {
                Ok(n) => n,
                Err(e) => return ReadResult::SerialError(e),
            };

            // Process each received byte
            for &byte in &read_buf[..bytes_read] {
                if let Some(frame) = self.accumulator.push(byte) {
                    // Frame complete, try to decode and parse
                    match self.process_frame(frame) {
                        Some(result) => return result,
                        None => continue, // Invalid frame, keep reading
                    }
                }
            }
        }
    }

    /// Process a complete COBS frame
    fn process_frame(&self, frame: heapless::Vec<u8, MAX_FRAME_SIZE>) -> Option<ReadResult> {
        // Decode COBS
        let decoded = match cobs_decode(&frame) {
            Ok(d) => d,
            Err(_) => return None, // Invalid COBS, discard
        };

        if decoded.is_empty() {
            return None;
        }

        // Extract command ID for error reporting
        let command_id = decoded[0];

        // Parse command
        match self.parser.parse(&decoded) {
            Ok(cmd) => Some(ReadResult::Command(cmd)),
            Err(status) => Some(ReadResult::ParseError(status, command_id)),
        }
    }

    /// Reset the reader state
    pub fn reset(&mut self) {
        self.accumulator.reset();
    }
}

impl Default for SerialCommandReader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::parser::calculate_crc;
    use crate::commands::serialiser::ResponseSerialiser;
    use crate::serial::traits::mock::MockSerialPort;

    /// Build a complete COBS-encoded frame for a command
    fn build_test_frame(cmd_id: u8, payload: &[u8]) -> heapless::Vec<u8, MAX_FRAME_SIZE> {
        // Build raw frame
        let mut raw: heapless::Vec<u8, MAX_FRAME_SIZE> = heapless::Vec::new();
        raw.push(cmd_id).unwrap();
        raw.extend_from_slice(&(payload.len() as u16).to_le_bytes())
            .unwrap();
        raw.extend_from_slice(payload).unwrap();

        let crc = calculate_crc(&raw);
        raw.extend_from_slice(&crc.to_le_bytes()).unwrap();

        // COBS encode
        let serialiser = ResponseSerialiser::new();
        let mut encoded: heapless::Vec<u8, MAX_FRAME_SIZE> = heapless::Vec::new();

        // Manual COBS encode for raw frame
        if raw.is_empty() {
            encoded.push(0x01).unwrap();
        } else {
            let mut code_idx = 0;
            encoded.push(0).unwrap(); // Placeholder

            let mut code: u8 = 1;

            for &byte in &raw {
                if byte == 0 {
                    encoded[code_idx] = code;
                    code_idx = encoded.len();
                    encoded.push(0).unwrap();
                    code = 1;
                } else {
                    encoded.push(byte).unwrap();
                    code += 1;

                    if code == 0xFF {
                        encoded[code_idx] = code;
                        code_idx = encoded.len();
                        encoded.push(0).unwrap();
                        code = 1;
                    }
                }
            }

            encoded[code_idx] = code;
        }

        // Add frame delimiter
        encoded.push(0x00).unwrap();

        encoded
    }

    #[test]
    fn test_read_get_version() {
        let mut reader = SerialCommandReader::new();
        let mut port = MockSerialPort::new();

        futures::executor::block_on(async {
            // Queue a GetVersion command frame
            let frame = build_test_frame(0x01, &[]);
            port.queue_rx_data(&frame);

            match reader.read_command(&mut port).await {
                ReadResult::Command(Command::GetVersion) => {}
                other => panic!("Expected GetVersion command, got {:?}", other),
            }
        });
    }

    #[test]
    fn test_read_lora_tx() {
        let mut reader = SerialCommandReader::new();
        let mut port = MockSerialPort::new();

        futures::executor::block_on(async {
            let payload = [0x48, 0x65, 0x6C, 0x6C, 0x6F]; // "Hello"
            let frame = build_test_frame(0x10, &payload);
            port.queue_rx_data(&frame);

            match reader.read_command(&mut port).await {
                ReadResult::Command(Command::LoraTx { data }) => {
                    assert_eq!(data.as_slice(), &payload);
                }
                other => panic!("Expected LoraTx command, got {:?}", other),
            }
        });
    }

    #[test]
    fn test_read_lora_rx() {
        let mut reader = SerialCommandReader::new();
        let mut port = MockSerialPort::new();

        futures::executor::block_on(async {
            let timeout: u32 = 5000;
            let frame = build_test_frame(0x11, &timeout.to_le_bytes());
            port.queue_rx_data(&frame);

            match reader.read_command(&mut port).await {
                ReadResult::Command(Command::LoraRx { timeout_ms }) => {
                    assert_eq!(timeout_ms, 5000);
                }
                other => panic!("Expected LoraRx command, got {:?}", other),
            }
        });
    }

    #[test]
    fn test_sequence_id() {
        let mut reader = SerialCommandReader::new();

        assert_eq!(reader.next_sequence_id(), 0);
        assert_eq!(reader.next_sequence_id(), 1);
        assert_eq!(reader.next_sequence_id(), 2);
    }

    #[test]
    fn test_multiple_frames() {
        let mut reader = SerialCommandReader::new();
        let mut port = MockSerialPort::new();

        futures::executor::block_on(async {
            // Queue two commands
            let frame1 = build_test_frame(0x01, &[]);
            let frame2 = build_test_frame(0x11, &1000u32.to_le_bytes());

            port.queue_rx_data(&frame1);
            port.queue_rx_data(&frame2);

            // Read first command
            match reader.read_command(&mut port).await {
                ReadResult::Command(Command::GetVersion) => {}
                other => panic!("Expected GetVersion, got {:?}", other),
            }

            // Read second command
            match reader.read_command(&mut port).await {
                ReadResult::Command(Command::LoraRx { timeout_ms }) => {
                    assert_eq!(timeout_ms, 1000);
                }
                other => panic!("Expected LoraRx, got {:?}", other),
            }
        });
    }
}
