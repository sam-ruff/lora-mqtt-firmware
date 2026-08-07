//! JSON and hex codecs for the MQTT payloads.
//!
//! Payload bytes travel as hex (256 B max -> 512 chars), which stays well
//! inside one socket buffer and reads cleanly in `mosquitto_sub`.

use heapless::{String, Vec};
use wt_protocol::MAX_LORA_PAYLOAD;

/// Enough for the uplink JSON with a full 256-byte payload as hex.
pub const MAX_JSON: usize = 640;

/// Hex form of a maximum-size LoRa payload.
pub type PayloadHex = String<{ MAX_LORA_PAYLOAD * 2 }>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// Not valid JSON or missing a required field.
    Json,
    /// payload_hex is not an even-length hex string within size limits.
    Hex,
}

pub fn hex_encode(data: &[u8]) -> PayloadHex {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::new();
    for byte in data.iter().take(MAX_LORA_PAYLOAD) {
        let _ = out.push(HEX[(byte >> 4) as usize] as char);
        let _ = out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

pub fn hex_decode(hex: &str) -> Result<Vec<u8, MAX_LORA_PAYLOAD>, CodecError> {
    if hex.is_empty() || !hex.len().is_multiple_of(2) || hex.len() > MAX_LORA_PAYLOAD * 2 {
        return Err(CodecError::Hex);
    }
    let mut out = Vec::new();
    let bytes = hex.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let high = hex_digit(pair[0]).ok_or(CodecError::Hex)?;
        let low = hex_digit(pair[1]).ok_or(CodecError::Hex)?;
        out.push((high << 4) | low).map_err(|_| CodecError::Hex)?;
    }
    Ok(out)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(serde::Serialize)]
struct UplinkMsg<'a> {
    payload_hex: &'a str,
    rssi: i16,
    snr: i8,
    seq: u32,
    uptime_ms: u64,
}

/// The `rx` topic JSON for a received LoRa packet.
pub fn uplink_json(
    data: &[u8],
    rssi: i16,
    snr: i8,
    seq: u32,
    uptime_ms: u64,
) -> Vec<u8, MAX_JSON> {
    let payload_hex = hex_encode(data);
    let msg = UplinkMsg { payload_hex: &payload_hex, rssi, snr, seq, uptime_ms };
    to_json(&msg)
}

#[derive(serde::Deserialize)]
struct DownlinkMsg<'a> {
    payload_hex: &'a str,
    id: Option<u32>,
}

/// A validated downlink request from the `tx` topic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downlink {
    pub payload: Vec<u8, MAX_LORA_PAYLOAD>,
    pub id: Option<u32>,
}

pub fn parse_downlink(json: &[u8]) -> Result<Downlink, CodecError> {
    let (msg, _) =
        serde_json_core::from_slice::<DownlinkMsg>(json).map_err(|_| CodecError::Json)?;
    let payload = hex_decode(msg.payload_hex)?;
    Ok(Downlink { payload, id: msg.id })
}

#[derive(serde::Serialize)]
struct TxResultMsg<'a> {
    result: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_secs: Option<u32>,
}

/// The `tx/result` topic JSON for a downlink outcome.
pub fn tx_result_json(
    result: &str,
    id: Option<u32>,
    retry_after_secs: Option<u32>,
) -> Vec<u8, MAX_JSON> {
    to_json(&TxResultMsg { result, id, retry_after_secs })
}

#[derive(serde::Serialize)]
struct StatusMsg<'a> {
    online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    fw: Option<&'a str>,
}

/// The retained `status` topic JSON. The offline variant doubles as the LWT.
pub fn status_json(online: bool, fw: Option<&str>) -> Vec<u8, MAX_JSON> {
    to_json(&StatusMsg { online, fw })
}

fn to_json<T: serde::Serialize>(value: &T) -> Vec<u8, MAX_JSON> {
    let mut buf = [0u8; MAX_JSON];
    // MAX_JSON is sized for the largest message; an overflow yields an empty
    // payload rather than a truncated one.
    let len = serde_json_core::to_slice(value, &mut buf).unwrap_or(0);
    let mut out = Vec::new();
    let _ = out.extend_from_slice(&buf[..len]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let data = [0x00, 0x01, 0xAB, 0xFF];
        let hex = hex_encode(&data);
        assert_eq!(hex.as_str(), "0001abff");
        assert_eq!(hex_decode(&hex).unwrap().as_slice(), &data);
    }

    #[test]
    fn hex_decode_rejects_bad_input() {
        assert_eq!(hex_decode(""), Err(CodecError::Hex));
        assert_eq!(hex_decode("abc"), Err(CodecError::Hex));
        assert_eq!(hex_decode("zz"), Err(CodecError::Hex));
        // 257 bytes exceeds MAX_LORA_PAYLOAD
        let long: std::string::String = "ab".repeat(257);
        assert_eq!(hex_decode(&long), Err(CodecError::Hex));
    }

    #[test]
    fn uplink_json_golden() {
        let json = uplink_json(b"Hi", -87, 5, 12, 123_456);
        assert_eq!(
            core::str::from_utf8(&json).unwrap(),
            r#"{"payload_hex":"4869","rssi":-87,"snr":5,"seq":12,"uptime_ms":123456}"#
        );
    }

    #[test]
    fn uplink_json_fits_max_payload() {
        let data = [0xAA; 256];
        let json = uplink_json(&data, -120, -19, u32::MAX, u64::MAX);
        assert!(!json.is_empty());
        assert!(json.len() <= MAX_JSON);
    }

    #[test]
    fn downlink_parses_with_and_without_id() {
        let down = parse_downlink(br#"{"payload_hex":"4869","id":7}"#).unwrap();
        assert_eq!(down.payload.as_slice(), b"Hi");
        assert_eq!(down.id, Some(7));

        let down = parse_downlink(br#"{"payload_hex":"ff"}"#).unwrap();
        assert_eq!(down.payload.as_slice(), &[0xFF]);
        assert_eq!(down.id, None);
    }

    #[test]
    fn downlink_rejects_bad_json_and_hex() {
        assert_eq!(parse_downlink(b"not json"), Err(CodecError::Json));
        assert_eq!(parse_downlink(br#"{"id":1}"#), Err(CodecError::Json));
        assert_eq!(
            parse_downlink(br#"{"payload_hex":"xyz1"}"#),
            Err(CodecError::Hex)
        );
        assert_eq!(parse_downlink(br#"{"payload_hex":""}"#), Err(CodecError::Hex));
    }

    #[test]
    fn tx_result_json_variants() {
        let sent = tx_result_json("sent", Some(7), None);
        assert_eq!(
            core::str::from_utf8(&sent).unwrap(),
            r#"{"result":"sent","id":7}"#
        );
        let refused = tx_result_json("refused", None, Some(30));
        assert_eq!(
            core::str::from_utf8(&refused).unwrap(),
            r#"{"result":"refused","retry_after_secs":30}"#
        );
    }

    #[test]
    fn status_json_variants() {
        assert_eq!(
            core::str::from_utf8(&status_json(true, Some("0.1.0"))).unwrap(),
            r#"{"online":true,"fw":"0.1.0"}"#
        );
        assert_eq!(
            core::str::from_utf8(&status_json(false, None)).unwrap(),
            r#"{"online":false}"#
        );
    }
}
