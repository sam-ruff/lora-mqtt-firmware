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
    LoraTx = 0x10,
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

/// Response IDs matching the firmware
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResponseId {
    Version = 0x01,
    TxComplete = 0x10,
    RxPacket = 0x11,
    Error = 0xFF,
}

impl TryFrom<u8> for ResponseId {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, <Self as TryFrom<u8>>::Error> {
        match value {
            0x01 => Ok(ResponseId::Version),
            0x10 => Ok(ResponseId::TxComplete),
            0x11 => Ok(ResponseId::RxPacket),
            0xFF => Ok(ResponseId::Error),
            _ => Err(value),
        }
    }
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
