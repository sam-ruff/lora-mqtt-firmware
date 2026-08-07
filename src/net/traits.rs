//! MQTT publishing seam.
//!
//! The bridge produces [`MqttOutbound`] values naming a logical [`Topic`];
//! [`deliver`] resolves the topic against the hub's [`TopicSet`] and hands
//! the bytes to an [`MqttSink`]. The production sink wraps the rust-mqtt
//! client in the MQTT task; tests use the recording mock.

use heapless::String;

use crate::bridge::{MqttOutbound, Topic};

/// Publishing failed; the connection should be torn down and retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkError;

// Single-threaded executor and all impls are in this crate, so Send bounds
// on the returned futures are not needed.
#[allow(async_fn_in_trait)]
pub trait MqttSink {
    async fn publish(
        &mut self,
        topic: &str,
        payload: &[u8],
        retain: bool,
    ) -> Result<(), SinkError>;
}

/// The hub's topic names, derived once from the device id.
pub struct TopicSet {
    rx: String<32>,
    tx: String<32>,
    tx_result: String<32>,
    status: String<32>,
}

impl TopicSet {
    /// Topics under `wt/hub/XXXXXX/` where `XXXXXX` is the device id in hex.
    pub fn new(device_id: [u8; 3]) -> Self {
        let mut base: String<16> = String::new();
        let _ = base.push_str("wt/hub/");
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for byte in device_id {
            let _ = base.push(HEX[(byte >> 4) as usize] as char);
            let _ = base.push(HEX[(byte & 0x0F) as usize] as char);
        }
        let with_suffix = |suffix: &str| {
            let mut topic: String<32> = String::new();
            let _ = topic.push_str(&base);
            let _ = topic.push_str(suffix);
            topic
        };
        Self {
            rx: with_suffix("/rx"),
            tx: with_suffix("/tx"),
            tx_result: with_suffix("/tx/result"),
            status: with_suffix("/status"),
        }
    }

    pub fn resolve(&self, topic: Topic) -> &str {
        match topic {
            Topic::Rx => &self.rx,
            Topic::TxResult => &self.tx_result,
        }
    }

    /// The downlink topic the MQTT task subscribes to.
    pub fn tx(&self) -> &str {
        &self.tx
    }

    pub fn status(&self) -> &str {
        &self.status
    }
}

/// Publish one queued outbound message through the sink.
pub async fn deliver(
    sink: &mut impl MqttSink,
    topics: &TopicSet,
    outbound: &MqttOutbound,
) -> Result<(), SinkError> {
    sink.publish(topics.resolve(outbound.topic), &outbound.payload, outbound.retain)
        .await
}

#[cfg(test)]
pub mod mock {
    use super::*;

    /// Records publishes; can be scripted to fail.
    #[derive(Default)]
    pub struct MockMqttSink {
        pub published: Vec<(std::string::String, std::vec::Vec<u8>, bool)>,
        pub fail_next: bool,
    }

    impl MqttSink for MockMqttSink {
        async fn publish(
            &mut self,
            topic: &str,
            payload: &[u8],
            retain: bool,
        ) -> Result<(), SinkError> {
            if self.fail_next {
                self.fail_next = false;
                return Err(SinkError);
            }
            self.published.push((topic.into(), payload.to_vec(), retain));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockMqttSink;
    use super::*;
    use crate::bridge::BridgeState;
    use futures::executor::block_on;

    #[test]
    fn topics_derive_from_device_id() {
        let topics = TopicSet::new([0xAA, 0xBB, 0xCC]);
        assert_eq!(topics.resolve(Topic::Rx), "wt/hub/AABBCC/rx");
        assert_eq!(topics.tx(), "wt/hub/AABBCC/tx");
        assert_eq!(topics.resolve(Topic::TxResult), "wt/hub/AABBCC/tx/result");
        assert_eq!(topics.status(), "wt/hub/AABBCC/status");
    }

    #[test]
    fn rx_packet_flows_to_the_rx_topic() {
        let mut state = BridgeState::new();
        let mut sink = MockMqttSink::default();
        let topics = TopicSet::new([0x01, 0x02, 0x03]);

        let outbound = state.on_rx_packet(b"Hi", -87, 5, 1000);
        block_on(deliver(&mut sink, &topics, &outbound)).unwrap();

        let (topic, payload, retain) = &sink.published[0];
        assert_eq!(topic, "wt/hub/010203/rx");
        assert!(core::str::from_utf8(payload).unwrap().contains("\"payload_hex\":\"4869\""));
        assert!(!retain);
    }

    #[test]
    fn sink_failure_propagates() {
        let mut state = BridgeState::new();
        let mut sink = MockMqttSink { fail_next: true, ..Default::default() };
        let topics = TopicSet::new([0, 0, 0]);
        let outbound = state.on_rx_packet(b"x", -50, 1, 0);
        assert_eq!(block_on(deliver(&mut sink, &topics, &outbound)), Err(SinkError));
        assert!(sink.published.is_empty());
    }
}
