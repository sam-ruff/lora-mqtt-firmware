//! LoRa task for radio operations with background listening
//!
//! Continuously listens for incoming LoRa packets and processes commands
//! when available, with a maximum latency defined by the RX poll interval.

use embassy_futures::select::{select, Either};

use crate::dispatcher::{CommandDispatcher, ResponseMessage, RESPONSE_CHANNEL};
use crate::lora::traits::LoraRadio;
use wt_protocol::{Command, Response};

use super::admin::{AdminCommand, ADMIN_CHANNEL};
use super::led::LedFlashDuration;
use super::serial::CommandReceiver;
use super::LedSender;

/// Background RX listen window. A command on COMMAND_CHANNEL cancels this early
/// (see the `select` below), so it only bounds how often RX is re-armed when
/// fully idle; it is no longer the command-response latency.
const RX_POLL_INTERVAL_MS: u32 = 500;

/// Task that handles LoRa operations with background listening
///
/// Waits concurrently on the radio (RX) and the command channel: whichever is
/// ready first wins, so an incoming host command is dispatched immediately
/// instead of after the RX poll, and the radio is listening whenever idle.
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
        // Listen for a packet and a host command at the same time. select drops
        // the losing future, so when a command arrives the in-flight receive() is
        // cancelled (radio stays in RX; the next transmit/receive takes over).
        match select(
            radio.receive(RX_POLL_INTERVAL_MS),
            command_receiver.receive(),
        )
        .await
        {
            Either::First(rx_result) => match rx_result {
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
                    response_pub.publish_immediate(ResponseMessage::Unsolicited(response));
                }
                // Timeout is the normal idle case; other errors just re-loop.
                Err(_) => {}
            },
            Either::Second(envelope) => {
                handle_command(&dispatcher, &mut radio, &led_sender, &response_pub, envelope).await;
            }
        }
    }
}

/// Dispatch a single host command and publish its response.
async fn handle_command<R: LoraRadio>(
    dispatcher: &CommandDispatcher,
    radio: &mut R,
    led_sender: &LedSender,
    response_pub: &crate::dispatcher::ResponsePublisher,
    envelope: crate::dispatcher::CommandEnvelope,
) {
    // Signal LED flash for command (non-blocking)
    let _ = led_sender.try_send(LedFlashDuration::Default);

    // Admin commands are handled by the admin task; no response is sent here.
    if let Command::Reboot = &envelope.command {
        let _ = ADMIN_CHANNEL.try_send(AdminCommand::Reboot);
        return;
    }

    // Log TX command if it's a LoraTx
    if let Command::LoraTx { ref data } = envelope.command {
        if let Ok(s) = core::str::from_utf8(data) {
            crate::debug!("LoRa TX: '{}'", s);
        } else {
            crate::debug!("LoRa TX: {} bytes", data.len());
        }
    }

    let response = dispatcher.dispatch(radio, envelope.command).await;

    // Log response
    match &response {
        Response::Version { major, minor, patch } => {
            crate::debug!("Version: {}.{}.{}", major, minor, patch);
        }
        Response::TxComplete => crate::debug!("LoRa TX: Complete"),
        Response::Error { status, .. } => crate::debug!("LoRa TX: Failed ({:?})", status),
        _ => {}
    }

    // Publish command response (subscribers filter by source)
    response_pub.publish_immediate(ResponseMessage::Command {
        source: envelope.source,
        sequence_id: envelope.sequence_id,
        response,
    });
}
