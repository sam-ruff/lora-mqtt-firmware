//! LoRa task for radio operations with background listening
//!
//! Continuously listens for incoming LoRa packets and processes commands
//! when available, with a maximum latency defined by the RX poll interval.

use crate::commands::Response;
use crate::debug;
use crate::dispatcher::{CommandDispatcher, ResponseMessage, RESPONSE_CHANNEL};
use crate::lora::traits::{LoraError, LoraRadio};

use super::led::LedFlashDuration;
use super::serial::CommandReceiver;
use super::LedSender;

/// Polling interval for background LoRa receive (max TX latency)
/// Higher SF requires longer time-on-air, so increase for SF11
const RX_POLL_INTERVAL_MS: u32 = 500;

/// Task that handles LoRa operations with background listening
///
/// This task continuously listens for incoming LoRa packets and pushes them
/// immediately to the host as unsolicited responses. Commands are processed
/// when available, with a maximum latency of RX_POLL_INTERVAL_MS.
pub async fn lora_task<R: LoraRadio>(
    mut radio: R,
    command_receiver: CommandReceiver,
    led_sender: LedSender,
) {
    let dispatcher = CommandDispatcher::new();

    // Get publisher for all responses (broadcasts to all subscribers)
    let response_pub = RESPONSE_CHANNEL.immediate_publisher();

    // Initialise LoRa radio
    crate::debug!("LoRa: Initialising radio...");
    match radio.init().await {
        Ok(()) => crate::debug!("LoRa: Radio initialised"),
        Err(_) => crate::debug!("LoRa: Radio init failed"),
    }

    loop {
        // Listen for LoRa packets with short timeout, checking for commands
        match radio.receive(RX_POLL_INTERVAL_MS).await {
            Ok(packet) => {
                // Signal LED flash for received packet (non-blocking)
                let _ = led_sender.try_send(LedFlashDuration::Default);

                // Log received packet (show as string if valid UTF-8, else hex)
                if let Ok(s) = core::str::from_utf8(&packet.data) {
                    crate::debug!("LoRa RX: '{}' (RSSI: {}, SNR: {})", s, packet.rssi, packet.snr);
                } else {
                    crate::debug!("LoRa RX: {} bytes (RSSI: {}, SNR: {})", packet.data.len(), packet.rssi, packet.snr);
                }

                let response = Response::RxPacket {
                    data: packet.data,
                    rssi: packet.rssi,
                    snr: packet.snr,
                };
                // Broadcast unsolicited to all subscribers (serial, BLE)
                let msg = ResponseMessage::Unsolicited(response);
                response_pub.publish_immediate(msg);
            }
            Err(LoraError::Timeout) => {
                // Normal - no packet received within poll interval
                // Check for pending commands during RX gap
            }
            Err(_) => {
                // Other errors - continue
            }
        }

        // Process any pending commands (non-blocking check)
        while let Ok(envelope) = command_receiver.try_receive() {
            // Signal LED flash for TX command (non-blocking)
            let _ = led_sender.try_send(LedFlashDuration::Default);

            // Log TX command if it's a LoraTx
            if let crate::commands::Command::LoraTx { ref data } = envelope.command {
                if let Ok(s) = core::str::from_utf8(data) {
                    crate::debug!("LoRa TX: '{}'", s);
                } else {
                    crate::debug!("LoRa TX: {} bytes", data.len());
                }
            }

            let response = dispatcher.dispatch(&mut radio, envelope.command).await;

            // Log TX result
            match &response {
                crate::commands::Response::TxComplete => {
                    crate::debug!("LoRa TX: Complete");
                }
                crate::commands::Response::Error { status, .. } => {
                    crate::debug!("LoRa TX: Failed ({:?})", status);
                }
                _ => {}
            }

            // Publish command response (subscribers filter by source)
            let msg = ResponseMessage::Command {
                source: envelope.source,
                sequence_id: envelope.sequence_id,
                response,
            };
            response_pub.publish_immediate(msg);
        }
        // Loop back to receive() - ensures we're always listening when idle
    }
}
