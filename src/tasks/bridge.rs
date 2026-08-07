//! Bridge task: connects the radio-side channels to the MQTT-side channels.
//!
//! Thin shell around [`BridgeState`]; all decisions live there and are
//! host-tested. Decoupled from the MQTT task so a stalled broker connection
//! can never make the response subscriber lag more than necessary.

use embassy_futures::select::{select, Either};
use embassy_sync::pubsub::WaitResult;
use embassy_time::Instant;
use wt_protocol::Response;

use crate::bridge::{BridgeState, DownlinkOutcome, MqttOutbound, MQTT_IN_CHANNEL, MQTT_OUT_CHANNEL};
use crate::dispatcher::{
    CommandEnvelope, CommandSource, ResponseMessage, COMMAND_CHANNEL, RESPONSE_CHANNEL,
};
use crate::net::stats::HUB_STATS;

pub async fn bridge_task() {
    let Ok(mut response_sub) = RESPONSE_CHANNEL.subscriber() else {
        crate::debug!("Bridge: no response subscriber slot available");
        return;
    };
    let command_sender = COMMAND_CHANNEL.sender();
    let mut state = BridgeState::new();

    loop {
        match select(response_sub.next_message(), MQTT_IN_CHANNEL.receive()).await {
            Either::First(WaitResult::Lagged(count)) => {
                crate::debug!("Bridge: response subscriber lagged, {} lost", count);
                HUB_STATS.count_dropped(count as u32);
            }
            Either::First(WaitResult::Message(message)) => match message {
                ResponseMessage::Unsolicited(Response::RxPacket { data, rssi, snr }) => {
                    let uptime_ms = Instant::now().as_millis();
                    let outbound = state.on_rx_packet(&data, rssi, snr, uptime_ms);
                    if enqueue(outbound) {
                        HUB_STATS.count_uplink();
                    }
                }
                ResponseMessage::Command {
                    source: CommandSource::WiFi,
                    sequence_id,
                    response,
                } => {
                    let sent = matches!(response, Response::TxComplete);
                    if let Some(outbound) = state.on_command_response(sequence_id, &response) {
                        if sent {
                            HUB_STATS.count_downlink();
                        }
                        enqueue(outbound);
                    }
                }
                _ => {}
            },
            Either::Second(json) => match state.on_downlink(&json) {
                DownlinkOutcome::Send { sequence_id, command } => {
                    let envelope = CommandEnvelope {
                        command,
                        source: CommandSource::WiFi,
                        sequence_id,
                    };
                    if command_sender.try_send(envelope).is_err() {
                        enqueue(state.abandon(sequence_id, "queue_full"));
                    }
                }
                DownlinkOutcome::Reject(outbound) => {
                    enqueue(outbound);
                }
            },
        }
    }
}

/// Queue for the MQTT task; drops (and counts) when the broker is behind.
fn enqueue(outbound: MqttOutbound) -> bool {
    if MQTT_OUT_CHANNEL.try_send(outbound).is_err() {
        HUB_STATS.count_dropped(1);
        return false;
    }
    true
}
