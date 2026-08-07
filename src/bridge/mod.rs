//! LoRa <-> MQTT bridge logic.
//!
//! [`BridgeState`] is the pure state machine: it turns radio responses into
//! MQTT publications and downlink JSON into LoRa commands, correlating
//! downlink ids with command sequence ids. The embedded task in
//! `tasks/bridge.rs` is a thin shell around it.

pub mod codec;

use heapless::{LinearMap, Vec};
use wt_protocol::{Command, Response};

use codec::{parse_downlink, tx_result_json, uplink_json, CodecError, MAX_JSON};

/// Destination topics, resolved to full strings by the MQTT task. The
/// status topic is not listed: the MQTT task publishes it directly at
/// connect time and via the LWT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Rx,
    TxResult,
}

/// A publication queued for the MQTT task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MqttOutbound {
    pub topic: Topic,
    pub payload: Vec<u8, MAX_JSON>,
    pub retain: bool,
}

/// Most concurrent downlinks awaiting a radio response.
const MAX_IN_FLIGHT: usize = 4;

/// What to do with a parsed downlink request.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // Boxing not available in embassy channels
pub enum DownlinkOutcome {
    /// Send this command with the given sequence id.
    Send { sequence_id: u16, command: Command },
    /// Refused before reaching the radio; publish this result instead.
    Reject(MqttOutbound),
}

pub struct BridgeState {
    /// Command sequence id -> optional downlink correlation id.
    in_flight: LinearMap<u16, Option<u32>, MAX_IN_FLIGHT>,
    next_sequence: u16,
    rx_seq: u32,
}

impl BridgeState {
    pub const fn new() -> Self {
        Self {
            in_flight: LinearMap::new(),
            next_sequence: 0,
            rx_seq: 0,
        }
    }

    /// A received LoRa packet becomes an `rx` topic publication.
    pub fn on_rx_packet(
        &mut self,
        data: &[u8],
        rssi: i16,
        snr: i8,
        uptime_ms: u64,
    ) -> MqttOutbound {
        let seq = self.rx_seq;
        self.rx_seq = self.rx_seq.wrapping_add(1);
        MqttOutbound {
            topic: Topic::Rx,
            payload: uplink_json(data, rssi, snr, seq, uptime_ms),
            retain: false,
        }
    }

    /// A downlink JSON from the `tx` topic becomes a LoRa command, or an
    /// immediate error result.
    pub fn on_downlink(&mut self, json: &[u8]) -> DownlinkOutcome {
        let downlink = match parse_downlink(json) {
            Ok(d) => d,
            Err(err) => {
                let reason = match err {
                    CodecError::Json => "bad_json",
                    CodecError::Hex => "bad_payload",
                };
                return DownlinkOutcome::Reject(tx_result(reason, None, None));
            }
        };

        if self.in_flight.len() == MAX_IN_FLIGHT {
            return DownlinkOutcome::Reject(tx_result("busy", downlink.id, None));
        }

        let sequence_id = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        // Capacity checked above, so this insert cannot fail.
        let _ = self.in_flight.insert(sequence_id, downlink.id);

        match Command::lora_tx(&downlink.payload) {
            Ok(command) => DownlinkOutcome::Send { sequence_id, command },
            Err(_) => {
                self.in_flight.remove(&sequence_id);
                DownlinkOutcome::Reject(tx_result("bad_payload", downlink.id, None))
            }
        }
    }

    /// Call when a queued downlink never reached the command channel, so its
    /// correlation entry does not leak.
    pub fn abandon(&mut self, sequence_id: u16, reason: &str) -> MqttOutbound {
        let id = self.in_flight.remove(&sequence_id).flatten();
        tx_result(reason, id, None)
    }

    /// A WiFi-sourced command response becomes a `tx/result` publication.
    pub fn on_command_response(
        &mut self,
        sequence_id: u16,
        response: &Response,
    ) -> Option<MqttOutbound> {
        let id = self.in_flight.remove(&sequence_id)?;
        let outbound = match response {
            Response::TxComplete => tx_result("sent", id, None),
            Response::TxRefused { retry_after_secs } => {
                tx_result("refused", id, Some(*retry_after_secs))
            }
            _ => tx_result("error", id, None),
        };
        Some(outbound)
    }
}

impl Default for BridgeState {
    fn default() -> Self {
        Self::new()
    }
}

fn tx_result(result: &str, id: Option<u32>, retry_after_secs: Option<u32>) -> MqttOutbound {
    MqttOutbound {
        topic: Topic::TxResult,
        payload: tx_result_json(result, id, retry_after_secs),
        retain: false,
    }
}

/// Raw downlink payloads from the MQTT task to the bridge task.
#[cfg(feature = "embedded")]
pub static MQTT_IN_CHANNEL: embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    Vec<u8, MAX_JSON>,
    4,
> = embassy_sync::channel::Channel::new();

/// Publications from the bridge task to the MQTT task. Bounded; when the
/// broker is unreachable the newest messages drop and are counted.
#[cfg(feature = "embedded")]
pub static MQTT_OUT_CHANNEL: embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    MqttOutbound,
    8,
> = embassy_sync::channel::Channel::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rx_packet_publishes_with_incrementing_seq() {
        let mut state = BridgeState::new();
        let first = state.on_rx_packet(b"a", -50, 3, 1000);
        let second = state.on_rx_packet(b"b", -50, 3, 2000);
        assert_eq!(first.topic, Topic::Rx);
        assert!(core::str::from_utf8(&first.payload).unwrap().contains("\"seq\":0"));
        assert!(core::str::from_utf8(&second.payload).unwrap().contains("\"seq\":1"));
    }

    #[test]
    fn downlink_send_and_complete_round_trip() {
        let mut state = BridgeState::new();
        let outcome = state.on_downlink(br#"{"payload_hex":"4869","id":42}"#);
        let DownlinkOutcome::Send { sequence_id, command } = outcome else {
            panic!("expected Send, got {outcome:?}");
        };
        assert_eq!(command, Command::lora_tx(b"Hi").unwrap());

        let result = state
            .on_command_response(sequence_id, &Response::TxComplete)
            .unwrap();
        assert_eq!(result.topic, Topic::TxResult);
        assert_eq!(
            core::str::from_utf8(&result.payload).unwrap(),
            r#"{"result":"sent","id":42}"#
        );
        // The entry is consumed; a second response is ignored.
        assert!(state.on_command_response(sequence_id, &Response::TxComplete).is_none());
    }

    #[test]
    fn refused_downlink_reports_retry_hint() {
        let mut state = BridgeState::new();
        let DownlinkOutcome::Send { sequence_id, .. } =
            state.on_downlink(br#"{"payload_hex":"ff"}"#)
        else {
            panic!("expected Send");
        };
        let result = state
            .on_command_response(sequence_id, &Response::TxRefused { retry_after_secs: 30 })
            .unwrap();
        assert_eq!(
            core::str::from_utf8(&result.payload).unwrap(),
            r#"{"result":"refused","retry_after_secs":30}"#
        );
    }

    #[test]
    fn invalid_downlink_is_rejected_without_state() {
        let mut state = BridgeState::new();
        let outcome = state.on_downlink(b"nonsense");
        let DownlinkOutcome::Reject(out) = outcome else {
            panic!("expected Reject");
        };
        assert!(core::str::from_utf8(&out.payload).unwrap().contains("bad_json"));
        // No in-flight entry was created.
        assert!(state.on_command_response(0, &Response::TxComplete).is_none());
    }

    #[test]
    fn in_flight_cap_rejects_busy() {
        let mut state = BridgeState::new();
        for _ in 0..MAX_IN_FLIGHT {
            let outcome = state.on_downlink(br#"{"payload_hex":"00"}"#);
            assert!(matches!(outcome, DownlinkOutcome::Send { .. }));
        }
        let outcome = state.on_downlink(br#"{"payload_hex":"00","id":9}"#);
        let DownlinkOutcome::Reject(out) = outcome else {
            panic!("expected Reject");
        };
        let json = core::str::from_utf8(&out.payload).unwrap();
        assert!(json.contains("busy"));
        assert!(json.contains("\"id\":9"));
    }

    #[test]
    fn abandon_clears_the_entry_and_reports() {
        let mut state = BridgeState::new();
        let DownlinkOutcome::Send { sequence_id, .. } =
            state.on_downlink(br#"{"payload_hex":"00","id":5}"#)
        else {
            panic!("expected Send");
        };
        let out = state.abandon(sequence_id, "queue_full");
        let json = core::str::from_utf8(&out.payload).unwrap();
        assert!(json.contains("queue_full"));
        assert!(json.contains("\"id\":5"));
        assert!(state.on_command_response(sequence_id, &Response::TxComplete).is_none());
    }

    #[test]
    fn responses_from_other_sources_are_ignored() {
        let mut state = BridgeState::new();
        // Sequence id 99 was never issued by the bridge.
        assert!(state.on_command_response(99, &Response::TxComplete).is_none());
    }
}
