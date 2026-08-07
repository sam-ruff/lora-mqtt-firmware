//! Hub extension of the Walkie Textie wire protocol.
//!
//! The hub speaks stock `wt-protocol` for everything a node firmware does
//! (LoraTx, RxPacket, GetVersion, Reboot). This crate adds the hub-only
//! provisioning and status commands in the reserved id block `0x20..=0x2F`,
//! using the identical frame layout: `[version:u8][id:u8][length:u16 LE]
//! [payload][crc16:u16 LE]`, CRC-16-XMODEM, COBS-encoded with a trailing
//! `0x00` delimiter.
//!
//! A receiver first tries `wt_protocol::parse_command`; when that fails with
//! `ResponseStatus::InvalidCommand` (unknown id) it tries
//! [`parse_command`] from this crate.
#![cfg_attr(not(test), no_std)]

pub mod codec;
pub mod types;

pub use codec::{encode_command, encode_response, parse_command, parse_response};
pub use types::{
    AckStatus, GatewaySettings, HubCommand, HubCommandId, HubMode, HubResponse, HubResponseId,
    HubStatus, LinkState, MqttSettings, WifiSettings,
};

/// Maximum WiFi SSID length in bytes.
pub const MAX_SSID_LEN: usize = 32;
/// Maximum WiFi password length in bytes.
pub const MAX_PASSWORD_LEN: usize = 64;
/// Maximum broker / network-server hostname length in bytes.
pub const MAX_HOST_LEN: usize = 64;
/// Maximum MQTT client id length in bytes.
pub const MAX_CLIENT_ID_LEN: usize = 32;
