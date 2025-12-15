//! Command dispatcher and channel definitions
//!
//! This module defines the channel architecture for multi-source command handling
//! and the dispatcher that executes commands.

use crate::commands::types::{Command, Response, ResponseStatus};
use crate::config::protocol;
use crate::lora::traits::{LoraError, LoraRadio};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;

/// Channel capacity for incoming commands
const COMMAND_CHANNEL_SIZE: usize = 8;

/// Identifies the source of a command for routing responses
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    /// Command received via serial/UART
    Serial,
    /// Command received via BLE
    Ble,
    /// Command received via WiFi (future)
    #[allow(dead_code)]
    WiFi,
}

/// Envelope wrapping a command with metadata
#[derive(Debug, Clone)]
pub struct CommandEnvelope {
    /// The actual command
    pub command: Command,
    /// Source of the command (for routing response)
    pub source: CommandSource,
    /// Sequence ID for matching responses to requests
    pub sequence_id: u16,
}

/// Message type for all outgoing responses
///
/// Subscribers filter based on message type:
/// - Command responses: filtered by source (only the originating interface receives it)
/// - Unsolicited: delivered to all connected interfaces
#[derive(Debug, Clone)]
pub enum ResponseMessage {
    /// Command response - should be filtered by source
    Command {
        source: CommandSource,
        #[allow(dead_code)]
        sequence_id: u16,
        response: Response,
    },
    /// Unsolicited packet (RxPacket) - delivered to all connected interfaces
    Unsolicited(Response),
}

/// Global channel for commands from all sources
///
/// Multiple producers (serial, BLE, WiFi) send commands here.
/// Single consumer (dispatcher) receives and executes them.
pub static COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, CommandEnvelope, COMMAND_CHANNEL_SIZE> =
    Channel::new();

/// Unified channel for all responses (command responses + unsolicited)
///
/// Uses PubSubChannel so multiple subscribers (serial, BLE) can receive messages.
/// Each subscriber filters based on ResponseMessage type:
/// - Command responses: only accepted if source matches the subscriber's interface
/// - Unsolicited: always accepted by all subscribers
///
/// Parameters: CAP=8 messages, SUBS=2 subscribers (serial, BLE), PUBS=1 publisher (lora_task)
pub static RESPONSE_CHANNEL: PubSubChannel<CriticalSectionRawMutex, ResponseMessage, 8, 2, 1> =
    PubSubChannel::new();

/// Command dispatcher
///
/// Receives commands from the channel and dispatches them to the appropriate
/// handler, returning responses via the appropriate response channel.
pub struct CommandDispatcher;

impl CommandDispatcher {
    /// Create a new command dispatcher
    pub fn new() -> Self {
        Self
    }

    /// Dispatch a command and return the response
    pub async fn dispatch<R: LoraRadio>(
        &self,
        radio: &mut R,
        command: Command,
    ) -> Response {
        match command {
            Command::GetVersion => self.handle_get_version(),
            Command::Reboot => {
                // Admin commands are handled by admin_task before reaching dispatcher
                // For non-embedded (tests), return an error
                Response::error(ResponseStatus::InvalidCommand, command.id())
            }
            Command::LoraTx { data } => self.handle_lora_tx(radio, &data).await,
        }
    }

    /// Handle GetVersion command
    fn handle_get_version(&self) -> Response {
        crate::debug!("Version requested. Responding {}.{}.{}", protocol::VERSION_MAJOR, protocol::VERSION_MINOR, protocol::VERSION_PATCH);
        Response::Version {
            major: protocol::VERSION_MAJOR,
            minor: protocol::VERSION_MINOR,
            patch: protocol::VERSION_PATCH,
        }
    }

    /// Handle LoraTx command
    async fn handle_lora_tx<R: LoraRadio>(&self, radio: &mut R, data: &[u8]) -> Response {
        match radio.transmit(data).await {
            Ok(()) => Response::TxComplete,
            Err(e) => self.lora_error_to_response(e, Command::LoraTx {
                data: heapless::Vec::new(),
            }),
        }
    }

    /// Convert a LoRa error to a response
    fn lora_error_to_response(&self, error: LoraError, command: Command) -> Response {
        let status = match error {
            LoraError::Timeout => ResponseStatus::Timeout,
            _ => ResponseStatus::LoraError,
        };
        Response::error(status, command.id())
    }
}

impl Default for CommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lora::traits::mock::MockLoraRadio;
    use heapless::Vec;

    #[test]
    fn test_dispatch_get_version() {
        let dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            let response = dispatcher.dispatch(&mut radio, Command::GetVersion).await;

            match response {
                Response::Version {
                    major,
                    minor,
                    patch,
                } => {
                    assert_eq!(major, protocol::VERSION_MAJOR);
                    assert_eq!(minor, protocol::VERSION_MINOR);
                    assert_eq!(patch, protocol::VERSION_PATCH);
                }
                _ => panic!("Expected Version response"),
            }
        });
    }

    #[test]
    fn test_dispatch_lora_tx() {
        let dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            radio.init().await.unwrap();

            let mut data = Vec::new();
            data.extend_from_slice(&[0x48, 0x65, 0x6C, 0x6C, 0x6F])
                .unwrap();

            let response = dispatcher
                .dispatch(&mut radio, Command::LoraTx { data: data.clone() })
                .await;

            assert!(matches!(response, Response::TxComplete));

            // Verify the data was transmitted
            let history = radio.get_tx_history();
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].as_slice(), data.as_slice());
        });
    }

    #[test]
    fn test_dispatch_lora_tx_error() {
        let dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            radio.set_next_tx_error(LoraError::TransmitFailed);

            let mut data = Vec::new();
            data.extend_from_slice(&[0x01]).unwrap();

            let response = dispatcher
                .dispatch(&mut radio, Command::LoraTx { data })
                .await;

            match response {
                Response::Error { status, .. } => {
                    assert_eq!(status, ResponseStatus::LoraError);
                }
                _ => panic!("Expected Error response"),
            }
        });
    }

}
