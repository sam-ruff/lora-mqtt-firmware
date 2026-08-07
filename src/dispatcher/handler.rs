//! Command dispatcher and channel definitions
//!
//! This module defines the channel architecture for multi-source command handling
//! and the dispatcher that executes commands.

use crate::config::protocol;
use crate::lora::airtime::time_on_air_ms;
use crate::lora::duty_cycle::DutyCycleLimiter;
use crate::lora::traits::{LoraConfig, LoraError, LoraRadio};
use wt_protocol::{Command, CommandId, Response, ResponseStatus};
#[cfg(feature = "embedded")]
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
#[cfg(feature = "embedded")]
use embassy_sync::channel::Channel;
#[cfg(feature = "embedded")]
use embassy_sync::pubsub::{ImmediatePublisher, PubSubChannel};

/// Channel capacity for incoming commands
#[cfg(feature = "embedded")]
const COMMAND_CHANNEL_SIZE: usize = 8;

/// Identifies the source of a command for routing responses
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    /// Command received via serial/UART
    Serial,
    /// Command received via WiFi (MQTT downlink)
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
/// Multiple producers (serial, WiFi) send commands here.
/// Single consumer (dispatcher) receives and executes them.
#[cfg(feature = "embedded")]
pub static COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, CommandEnvelope, COMMAND_CHANNEL_SIZE> =
    Channel::new();

/// Unified channel for all responses (command responses + unsolicited)
///
/// Uses PubSubChannel so multiple subscribers (serial, MQTT bridge) can receive
/// messages. Each subscriber filters based on ResponseMessage type:
/// - Command responses: only accepted if source matches the subscriber's interface
/// - Unsolicited: always accepted by all subscribers
///
/// Parameters: CAP=8 messages, SUBS=2 subscribers (serial, MQTT bridge), PUBS=1 publisher (lora_task)
#[cfg(feature = "embedded")]
pub static RESPONSE_CHANNEL: PubSubChannel<CriticalSectionRawMutex, ResponseMessage, 8, 2, 1> =
    PubSubChannel::new();

/// Immediate publisher for `RESPONSE_CHANNEL` (the LoRa task broadcasts here).
#[cfg(feature = "embedded")]
pub type ResponsePublisher =
    ImmediatePublisher<'static, CriticalSectionRawMutex, ResponseMessage, 8, 2, 1>;

/// Command dispatcher
///
/// Receives commands from the channel and dispatches them to the appropriate
/// handler, returning responses via the appropriate response channel. Owns the
/// active radio configuration and the regulatory duty cycle budget.
pub struct CommandDispatcher {
    config: LoraConfig,
    duty_cycle: DutyCycleLimiter,
}

impl CommandDispatcher {
    /// Create a new command dispatcher with the default radio configuration.
    pub fn new() -> Self {
        let config = LoraConfig::default();
        let duty_cycle = DutyCycleLimiter::for_frequency(config.frequency_hz);
        Self { config, duty_cycle }
    }

    /// Dispatch a command and return the response.
    ///
    /// `now_ms` is a monotonic timestamp (ms since boot) used for duty cycle
    /// accounting; injecting it keeps the dispatcher clock-free and testable.
    pub async fn dispatch<R: LoraRadio>(
        &mut self,
        radio: &mut R,
        command: Command,
        now_ms: u64,
    ) -> Response {
        match command {
            Command::GetVersion => self.handle_get_version(),
            Command::Reboot => {
                // Admin commands are handled by admin_task before reaching dispatcher
                // For non-embedded (tests), return an error
                Response::error(ResponseStatus::InvalidCommand, command.id())
            }
            Command::LoraTx { data } => self.handle_lora_tx(radio, &data, now_ms).await,
            Command::SetSpreadingFactor { spreading_factor } => {
                self.handle_set_spreading_factor(radio, spreading_factor).await
            }
            Command::GetRadioConfig => self.radio_config_response(),
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
    async fn handle_lora_tx<R: LoraRadio>(
        &mut self,
        radio: &mut R,
        data: &[u8],
        now_ms: u64,
    ) -> Response {
        let airtime_ms = time_on_air_ms(
            self.config.spreading_factor,
            self.config.bandwidth_khz,
            self.config.coding_rate,
            data.len(),
        );
        // Claim the budget before transmitting; a failed transmit stays
        // charged, which errs on the compliant side.
        if let Err(exceeded) = self.duty_cycle.try_claim(now_ms, airtime_ms) {
            let retry_after_secs = match exceeded.retry_after_ms {
                Some(ms) => ms.div_ceil(1000).min(u32::MAX as u64 - 1) as u32,
                // The packet exceeds the entire hourly budget.
                None => u32::MAX,
            };
            crate::debug!("LoRa TX refused: duty cycle, retry in {} s", retry_after_secs);
            return Response::TxRefused { retry_after_secs };
        }

        match radio.transmit(data).await {
            Ok(()) => Response::TxComplete,
            Err(e) => self.lora_error_to_response(e, CommandId::LoraTx),
        }
    }

    /// Handle SetSpreadingFactor: validate, reconfigure the radio, report back.
    async fn handle_set_spreading_factor<R: LoraRadio>(
        &mut self,
        radio: &mut R,
        spreading_factor: u8,
    ) -> Response {
        if !(7..=12).contains(&spreading_factor) {
            return Response::error(
                ResponseStatus::InvalidParameter,
                CommandId::SetSpreadingFactor,
            );
        }

        let mut config = self.config.clone();
        config.spreading_factor = spreading_factor;
        if let Err(e) = radio.configure(&config).await {
            return self.lora_error_to_response(e, CommandId::SetSpreadingFactor);
        }

        self.config = config;
        crate::debug!("Spreading factor set to SF{}", spreading_factor);
        self.radio_config_response()
    }

    /// The active configuration as a RadioConfig response.
    fn radio_config_response(&self) -> Response {
        Response::RadioConfig {
            frequency_hz: self.config.frequency_hz,
            spreading_factor: self.config.spreading_factor,
            bandwidth_khz: self.config.bandwidth_khz,
            coding_rate: self.config.coding_rate,
            tx_power_dbm: self.config.tx_power_dbm,
        }
    }

    /// Convert a LoRa error to a response
    fn lora_error_to_response(&self, error: LoraError, command_id: CommandId) -> Response {
        let status = match error {
            LoraError::Timeout => ResponseStatus::Timeout,
            // A rejected payload (empty or beyond the modem's 255-byte limit)
            LoraError::InvalidConfig => ResponseStatus::InvalidLength,
            _ => ResponseStatus::LoraError,
        };
        Response::error(status, command_id)
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
        let mut dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            let response = dispatcher.dispatch(&mut radio, Command::GetVersion, 0).await;

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
        let mut dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            radio.init().await.unwrap();

            let mut data = Vec::new();
            data.extend_from_slice(&[0x48, 0x65, 0x6C, 0x6C, 0x6F])
                .unwrap();

            let response = dispatcher
                .dispatch(&mut radio, Command::LoraTx { data: data.clone() }, 0)
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
        let mut dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            radio.set_next_tx_error(LoraError::TransmitFailed);

            let mut data = Vec::new();
            data.extend_from_slice(&[0x01]).unwrap();

            let response = dispatcher
                .dispatch(&mut radio, Command::LoraTx { data }, 0)
                .await;

            match response {
                Response::Error { status, .. } => {
                    assert_eq!(status, ResponseStatus::LoraError);
                }
                _ => panic!("Expected Error response"),
            }
        });
    }

    #[test]
    fn get_radio_config_reports_sf11_default() {
        let mut dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            let response = dispatcher
                .dispatch(&mut radio, Command::GetRadioConfig, 0)
                .await;

            match response {
                Response::RadioConfig { frequency_hz, spreading_factor, .. } => {
                    assert_eq!(spreading_factor, 11, "default must be SF11");
                    assert_eq!(frequency_hz, 869_525_000);
                }
                _ => panic!("Expected RadioConfig response"),
            }
        });
    }

    #[test]
    fn set_spreading_factor_reconfigures_the_radio() {
        let mut dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            let response = dispatcher
                .dispatch(
                    &mut radio,
                    Command::SetSpreadingFactor { spreading_factor: 7 },
                    0,
                )
                .await;

            match response {
                Response::RadioConfig { spreading_factor, .. } => {
                    assert_eq!(spreading_factor, 7);
                }
                _ => panic!("Expected RadioConfig response"),
            }

            let applied = radio.get_config().expect("radio must be reconfigured");
            assert_eq!(applied.spreading_factor, 7);
            // Everything else keeps the defaults.
            assert_eq!(applied.frequency_hz, 869_525_000);
            assert_eq!(applied.bandwidth_khz, 250);
        });
    }

    #[test]
    fn out_of_range_spreading_factor_is_rejected() {
        let mut dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            for sf in [0, 6, 13, 255] {
                let response = dispatcher
                    .dispatch(
                        &mut radio,
                        Command::SetSpreadingFactor { spreading_factor: sf },
                        0,
                    )
                    .await;
                match response {
                    Response::Error { status, .. } => {
                        assert_eq!(status, ResponseStatus::InvalidParameter);
                    }
                    _ => panic!("SF{sf} must be rejected"),
                }
            }
            assert!(radio.get_config().is_none(), "rejected SF must not touch the radio");
        });
    }

    #[test]
    fn set_spreading_factor_keeps_old_config_on_radio_error() {
        let mut dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            radio.set_next_configure_error(LoraError::SpiError);
            let response = dispatcher
                .dispatch(
                    &mut radio,
                    Command::SetSpreadingFactor { spreading_factor: 9 },
                    0,
                )
                .await;
            assert!(matches!(response, Response::Error { .. }));

            // The reported config still holds the default SF.
            let response = dispatcher
                .dispatch(&mut radio, Command::GetRadioConfig, 0)
                .await;
            match response {
                Response::RadioConfig { spreading_factor, .. } => {
                    assert_eq!(spreading_factor, 11);
                }
                _ => panic!("Expected RadioConfig response"),
            }
        });
    }

    /// Fill the duty cycle budget with max-size packets, then verify refusal.
    #[test]
    fn lora_tx_is_refused_once_the_duty_cycle_budget_is_spent() {
        let mut dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            radio.init().await.unwrap();

            let mut data = Vec::new();
            data.extend_from_slice(&[0xAA; 255]).unwrap();

            // A 255-byte packet at SF11/BW250/CR4/8 is ~3.25 s on air; the 10%
            // band allows 360 s per hour, so at most 110 fit.
            let mut sent = 0;
            let mut refused_at = None;
            for i in 0..130u64 {
                let response = dispatcher
                    .dispatch(&mut radio, Command::LoraTx { data: data.clone() }, i * 100)
                    .await;
                match response {
                    Response::TxComplete => sent += 1,
                    Response::TxRefused { retry_after_secs } => {
                        assert!(retry_after_secs > 0, "refusal must carry a retry hint");
                        assert!(retry_after_secs <= 3_600, "retry must be within the hour");
                        refused_at = Some(i);
                        break;
                    }
                    _ => panic!("Unexpected response"),
                }
            }

            assert_eq!(refused_at, Some(sent), "refusal must follow the last success");
            assert!((108..=112).contains(&sent), "got {sent} packets through");

            // A sliding hour later the budget is free again.
            let response = dispatcher
                .dispatch(
                    &mut radio,
                    Command::LoraTx { data: data.clone() },
                    61 * 60_000,
                )
                .await;
            assert!(matches!(response, Response::TxComplete));
        });
    }

    /// Dropping to SF7 makes packets ~40x cheaper, so far more of them fit.
    #[test]
    fn duty_cycle_budget_follows_the_configured_spreading_factor() {
        let mut dispatcher = CommandDispatcher::new();
        let mut radio = MockLoraRadio::new();

        futures::executor::block_on(async {
            radio.init().await.unwrap();

            dispatcher
                .dispatch(
                    &mut radio,
                    Command::SetSpreadingFactor { spreading_factor: 7 },
                    0,
                )
                .await;

            let mut data = Vec::new();
            data.extend_from_slice(&[0xAA; 255]).unwrap();

            // ~184 ms per packet: well over 60 must fit in the 360 s budget.
            for i in 0..60u64 {
                let response = dispatcher
                    .dispatch(&mut radio, Command::LoraTx { data: data.clone() }, i * 100)
                    .await;
                assert!(matches!(response, Response::TxComplete));
            }
        });
    }
}
