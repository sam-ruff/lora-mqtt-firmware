//! Protocol definitions matching the firmware.

#![allow(dead_code)]

use crc::{Crc, CRC_16_XMODEM};

/// Protocol version (must match firmware)
pub const PROTOCOL_VERSION: u8 = 1;

/// Command IDs matching the firmware protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CommandId {
    GetVersion = 0x01,
    Reboot = 0x03,
    LoraTx = 0x10,
    SetSpreadingFactor = 0x11,
    GetRadioConfig = 0x12,
}

/// Hub command IDs (reserved block 0x20..=0x2F).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HubCommandId {
    SetWifiConfig = 0x20,
    SetMqttConfig = 0x21,
    GetNetworkConfig = 0x22,
    GetHubStatus = 0x23,
    SetMode = 0x24,
    ClearNetworkConfig = 0x25,
    SetGatewayConfig = 0x26,
}

/// Response status codes matching the firmware protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResponseStatus {
    Success = 0x00,
    InvalidCommand = 0x01,
    InvalidLength = 0x02,
    CrcError = 0x03,
    InvalidVersion = 0x04,
    InvalidParameter = 0x05,
    LoraError = 0x10,
    Timeout = 0x11,
}

impl TryFrom<u8> for ResponseStatus {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(ResponseStatus::Success),
            0x01 => Ok(ResponseStatus::InvalidCommand),
            0x02 => Ok(ResponseStatus::InvalidLength),
            0x03 => Ok(ResponseStatus::CrcError),
            0x04 => Ok(ResponseStatus::InvalidVersion),
            0x05 => Ok(ResponseStatus::InvalidParameter),
            0x10 => Ok(ResponseStatus::LoraError),
            0x11 => Ok(ResponseStatus::Timeout),
            _ => Err(value),
        }
    }
}

const CRC: Crc<u16> = Crc::<u16>::new(&CRC_16_XMODEM);

/// Build a command frame (without COBS encoding).
/// Format: [version: u8][cmd_id: u8][length: u16 LE][payload][crc16: u16 LE]
pub fn build_command_payload(cmd_id: u8, payload: &[u8]) -> Vec<u8> {
    let length = payload.len() as u16;
    let mut data = Vec::with_capacity(6 + payload.len());

    data.push(PROTOCOL_VERSION);
    data.push(cmd_id);
    data.extend_from_slice(&length.to_le_bytes());
    data.extend_from_slice(payload);

    let checksum = CRC.checksum(&data);
    data.extend_from_slice(&checksum.to_le_bytes());

    data
}

/// COBS encode (corncobs includes zero delimiter).
pub fn cobs_encode(data: &[u8]) -> Vec<u8> {
    let mut encoded = vec![0u8; corncobs::max_encoded_len(data.len())];
    let len = corncobs::encode_buf(data, &mut encoded);
    encoded.truncate(len);
    // corncobs::encode_buf already includes the trailing zero delimiter
    encoded
}

/// Build a complete COBS-encoded command frame.
pub fn build_command(cmd_id: CommandId, payload: &[u8]) -> Vec<u8> {
    let raw = build_command_payload(cmd_id as u8, payload);
    cobs_encode(&raw)
}

/// Response IDs matching the firmware (stock plus the hub block).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResponseId {
    Version = 0x01,
    TxComplete = 0x10,
    RxPacket = 0x11,
    RadioConfig = 0x12,
    TxRefused = 0x13,
    NetworkConfig = 0x20,
    HubStatus = 0x21,
    ConfigAck = 0x22,
    Error = 0xFF,
}

impl TryFrom<u8> for ResponseId {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, <Self as TryFrom<u8>>::Error> {
        match value {
            0x01 => Ok(ResponseId::Version),
            0x10 => Ok(ResponseId::TxComplete),
            0x11 => Ok(ResponseId::RxPacket),
            0x12 => Ok(ResponseId::RadioConfig),
            0x13 => Ok(ResponseId::TxRefused),
            0x20 => Ok(ResponseId::NetworkConfig),
            0x21 => Ok(ResponseId::HubStatus),
            0x22 => Ok(ResponseId::ConfigAck),
            0xFF => Ok(ResponseId::Error),
            _ => Err(value),
        }
    }
}

/// Payload builders for the hub provisioning commands. Strings are one-byte
/// length prefixed, integers little-endian, matching hub-protocol.
pub fn wifi_config_payload(ssid: &str, password: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    push_string(&mut payload, ssid);
    push_string(&mut payload, password);
    payload
}

pub fn mqtt_config_payload(host: &str, port: u16, client_id: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&port.to_le_bytes());
    push_string(&mut payload, host);
    push_string(&mut payload, client_id);
    payload
}

pub fn gateway_config_payload(
    freq_hz: u32,
    spreading_factor: u8,
    bandwidth_khz: u32,
    coding_rate: u8,
    host: &str,
    port: u16,
) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&freq_hz.to_le_bytes());
    payload.push(spreading_factor);
    payload.extend_from_slice(&bandwidth_khz.to_le_bytes());
    payload.push(coding_rate);
    payload.extend_from_slice(&port.to_le_bytes());
    push_string(&mut payload, host);
    payload
}

fn push_string(payload: &mut Vec<u8>, s: &str) {
    payload.push(s.len() as u8);
    payload.extend_from_slice(s.as_bytes());
}

/// Parsed HubStatus response payload.
#[derive(Debug, Clone, Copy)]
pub struct HubStatus {
    pub wifi_state: u8,
    pub ip: [u8; 4],
    pub mqtt_state: u8,
    pub uplink_count: u32,
    pub downlink_count: u32,
    pub dropped_count: u32,
    pub uptime_secs: u32,
}

/// Link states as reported in HubStatus.
pub mod link_state {
    pub const UNPROVISIONED: u8 = 0;
    pub const CONNECTING: u8 = 1;
    pub const CONNECTED: u8 = 2;
    pub const DISCONNECTED: u8 = 3;
}

pub fn parse_hub_status(payload: &[u8]) -> anyhow::Result<HubStatus> {
    if payload.len() != 22 {
        anyhow::bail!("HubStatus payload must be 22 bytes, got {}", payload.len());
    }
    Ok(HubStatus {
        wifi_state: payload[0],
        ip: [payload[1], payload[2], payload[3], payload[4]],
        mqtt_state: payload[5],
        uplink_count: u32::from_le_bytes([payload[6], payload[7], payload[8], payload[9]]),
        downlink_count: u32::from_le_bytes([payload[10], payload[11], payload[12], payload[13]]),
        dropped_count: u32::from_le_bytes([payload[14], payload[15], payload[16], payload[17]]),
        uptime_secs: u32::from_le_bytes([payload[18], payload[19], payload[20], payload[21]]),
    })
}

/// Parsed response from the device.
#[derive(Debug)]
pub struct Response {
    pub version: u8,
    pub resp_id: ResponseId,
    pub payload: Vec<u8>,
}

/// Parse a COBS-decoded response.
/// Format: [version: u8][resp_id: u8][length: u16 LE][payload][crc: u16 LE]
pub fn parse_response(data: &[u8]) -> anyhow::Result<Response> {
    if data.len() < 6 {
        anyhow::bail!("Response too short: {} bytes", data.len());
    }

    let version = data[0];
    let resp_id_byte = data[1];
    let length = u16::from_le_bytes([data[2], data[3]]) as usize;

    if data.len() < 4 + length + 2 {
        anyhow::bail!(
            "Response payload incomplete: expected {}, got {}",
            4 + length + 2,
            data.len()
        );
    }

    let payload = data[4..4 + length].to_vec();
    let received_crc = u16::from_le_bytes([data[4 + length], data[4 + length + 1]]);

    // Verify CRC over version + resp_id + length + payload
    let calculated_crc = CRC.checksum(&data[..4 + length]);
    if calculated_crc != received_crc {
        anyhow::bail!(
            "CRC mismatch: expected {:04x}, got {:04x}",
            calculated_crc,
            received_crc
        );
    }

    // Verify protocol version
    if version != PROTOCOL_VERSION {
        anyhow::bail!(
            "Protocol version mismatch: expected {}, got {}",
            PROTOCOL_VERSION,
            version
        );
    }

    let resp_id = ResponseId::try_from(resp_id_byte)
        .map_err(|v| anyhow::anyhow!("Unknown response ID: {:#04x}", v))?;

    Ok(Response {
        version,
        resp_id,
        payload,
    })
}

/// COBS decode a frame (without the zero delimiter).
pub fn cobs_decode(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut decoded = vec![0u8; data.len()];
    let len = corncobs::decode_buf(data, &mut decoded)
        .map_err(|e| anyhow::anyhow!("COBS decode error: {:?}", e))?;
    decoded.truncate(len);
    Ok(decoded)
}
