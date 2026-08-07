//! Single-channel LoRaWAN gateway (Semtech UDP packet forwarder).
//!
//! The gateway is a payload-agnostic packet forwarder: it relays LoRa frames
//! between the radio and a LoRaWAN network server (ChirpStack's gateway
//! bridge speaking the Semtech UDP protocol on port 1700). MIC verification,
//! joins, frame counters and decryption all happen at the network server -
//! no LoRaWAN stack crate belongs in this firmware.
//!
//! Single-radio realities, accepted by design: only uplinks on the one
//! configured frequency/spreading-factor are heard, and the radio is deaf
//! while transmitting a downlink.

pub mod eui;
pub mod forwarder;
pub mod scheduler;
pub mod udp_protocol;

use heapless::Vec;

use crate::lora::traits::LoraConfig;

/// A downlink accepted from the network server, awaiting transmission.
#[derive(Debug, Clone, PartialEq)]
pub struct DownlinkJob {
    /// Target microsecond counter value, or `None` for immediate (Class C).
    pub target_tmst: Option<u32>,
    /// Full radio configuration for this transmission (frequency, data rate,
    /// power, inverted IQ, CRC off).
    pub config: LoraConfig,
    pub payload: Vec<u8, 256>,
}

/// Uplinks from the radio task to the UDP task.
#[cfg(feature = "embedded")]
pub static GW_UPLINK_CHANNEL: embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    (udp_protocol::RxMeta, Vec<u8, 256>),
    4,
> = embassy_sync::channel::Channel::new();

/// Accepted downlink jobs from the UDP task to the radio task.
#[cfg(feature = "embedded")]
pub static GW_DOWNLINK_CHANNEL: embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    DownlinkJob,
    2,
> = embassy_sync::channel::Channel::new();
