//! Serial tasks for command/response handling.
//!
//! Handles reading commands from and writing responses to a serial interface.
//! These tasks are generic over any type implementing embedded_io_async traits,
//! allowing them to work with USB Serial JTAG, USB CDC-ACM, or other serial interfaces.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Receiver, Sender};
use embedded_io_async::{Read, Write};

use crate::config;
use crate::dispatcher::{
    CommandEnvelope, CommandSource, HubCommandEnvelope, ResponseMessage, HUB_CHANNEL,
    RESPONSE_CHANNEL,
};
use hub_protocol::HubCommand;
use wt_protocol::{Command, FrameAccumulator, Response, ResponseStatus};

/// Result of attempting to parse a frame
enum ReadResult {
    /// Successfully parsed a command
    Command(Command),
    /// Successfully parsed a hub provisioning/status command
    HubCommand(HubCommand),
    /// Parse error (should send error response)
    ParseError(ResponseStatus, u8),
}

/// Type alias for the command channel sender
pub type CommandSender = Sender<'static, CriticalSectionRawMutex, CommandEnvelope, 8>;

/// Type alias for the command channel receiver
pub type CommandReceiver = Receiver<'static, CriticalSectionRawMutex, CommandEnvelope, 8>;

/// Task that reads commands from a serial interface.
///
/// Generic over any type implementing `embedded_io_async::Read`.
pub async fn serial_reader_task<R: Read>(
    mut reader: R,
    command_sender: CommandSender,
) {
    let mut accumulator = FrameAccumulator::new();
    let mut sequence_counter: u16 = 0;

    // Get publisher for sending parse error responses
    let response_pub = RESPONSE_CHANNEL.immediate_publisher();

    loop {
        // Read bytes from serial
        let mut buf = [0u8; 64];
        match reader.read(&mut buf).await {
            Ok(0) => continue,
            Ok(n) => {
                // Process each byte through the frame accumulator
                for &byte in &buf[..n] {
                    if let Some(frame) = accumulator.push(byte) {
                        // Frame complete, try to decode and parse
                        let seq_id = sequence_counter;
                        sequence_counter = sequence_counter.wrapping_add(1);

                        match process_frame(frame) {
                            Some(ReadResult::Command(cmd)) => {
                                let envelope = CommandEnvelope {
                                    command: cmd,
                                    source: CommandSource::Serial,
                                    sequence_id: seq_id,
                                };
                                command_sender.send(envelope).await;
                            }
                            Some(ReadResult::HubCommand(cmd)) => {
                                let envelope = HubCommandEnvelope {
                                    command: cmd,
                                    source: CommandSource::Serial,
                                    sequence_id: seq_id,
                                };
                                if HUB_CHANNEL.try_send(envelope).is_err() {
                                    crate::debug!("Serial: hub command queue full");
                                }
                            }
                            Some(ReadResult::ParseError(status, cmd_id)) => {
                                let response = Response::error_raw(status, cmd_id);
                                let msg = ResponseMessage::Command {
                                    source: CommandSource::Serial,
                                    sequence_id: seq_id,
                                    response,
                                };
                                response_pub.publish_immediate(msg);
                            }
                            None => {
                                // Invalid frame, ignore
                            }
                        }
                    }
                }
            }
            Err(_) => {
                // UART error, just continue
                embassy_time::Timer::after(embassy_time::Duration::from_millis(10)).await;
            }
        }
    }
}

/// Process a complete COBS frame (delimiter included).
///
/// Two-stage parse: stock wt-protocol commands first; on an unknown id the
/// hub command block (0x20..=0x2F) is tried before reporting an error.
fn process_frame(
    frame: heapless::Vec<u8, { config::protocol::MAX_FRAME_SIZE }>,
) -> Option<ReadResult> {
    let decoded = match wt_protocol::cobs_decode(&frame) {
        Ok(d) => d,
        Err(_) => return None,
    };

    if decoded.is_empty() {
        return None;
    }

    // Byte 1 is the command id (byte 0 is the protocol version); echoed back on error.
    let command_id = decoded.get(1).copied().unwrap_or(0);

    match wt_protocol::parse_command(&decoded) {
        Ok(cmd) => Some(ReadResult::Command(cmd)),
        Err(ResponseStatus::InvalidCommand) => match hub_protocol::parse_command(&decoded) {
            Ok(cmd) => Some(ReadResult::HubCommand(cmd)),
            Err(status) => Some(ReadResult::ParseError(status, command_id)),
        },
        Err(status) => Some(ReadResult::ParseError(status, command_id)),
    }
}

/// Task that writes responses to a serial interface.
///
/// Generic over any type implementing `embedded_io_async::Write`.
pub async fn serial_writer_task<W: Write>(mut writer: W) {
    // Subscribe to unified response channel
    let mut response_sub = RESPONSE_CHANNEL.subscriber().unwrap();

    loop {
        let msg = match response_sub.next_message().await {
            // The serial link fell behind the publisher and messages were
            // dropped; make that visible instead of silent.
            embassy_sync::pubsub::WaitResult::Lagged(count) => {
                crate::debug!("Serial: response subscriber lagged, {} lost", count);
                continue;
            }
            embassy_sync::pubsub::WaitResult::Message(msg) => msg,
        };

        // Filter and process messages
        let response = match msg {
            ResponseMessage::Command { source, response, .. } => {
                // Only process responses for Serial source
                if source == CommandSource::Serial {
                    Some(response)
                } else {
                    None
                }
            }
            ResponseMessage::Unsolicited(response) => {
                // Always process unsolicited packets
                Some(response)
            }
            ResponseMessage::HubRaw { source, frame } => {
                // Pre-encoded hub-protocol frame; write it verbatim
                if source == CommandSource::Serial {
                    let _ = writer.write_all(&frame).await;
                }
                None
            }
        };

        if let Some(response) = response {
            let encoded = wt_protocol::encode_response(&response);
            let _ = writer.write_all(&encoded).await;
        }
    }
}
