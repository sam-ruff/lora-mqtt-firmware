//! Frame encode/decode for the hub command block.
//!
//! Same wire format as `wt-protocol`: `[version:u8][id:u8][length:u16 LE]
//! [payload][crc16:u16 LE]`, CRC-16-XMODEM over everything before the CRC,
//! then COBS-encoded with a trailing `0x00` delimiter. Strings are length
//! prefixed with a single byte.

use heapless::{String, Vec};
use wt_protocol::{crc16, ParseError, ResponseStatus, MAX_FRAME_SIZE, PROTOCOL_VERSION};

use crate::types::{
    AckStatus, GatewaySettings, HubCommand, HubCommandId, HubMode, HubResponse, HubResponseId,
    HubStatus, LinkState, MqttSettings, WifiSettings,
};

/// Build a raw (un-COBS) frame: version, id, length, payload, CRC.
fn build_raw(id: u8, payload: &[u8]) -> Vec<u8, MAX_FRAME_SIZE> {
    let mut frame: Vec<u8, MAX_FRAME_SIZE> = Vec::new();
    let _ = frame.push(PROTOCOL_VERSION);
    let _ = frame.push(id);
    let _ = frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    let _ = frame.extend_from_slice(payload);
    let crc = crc16(&frame);
    let _ = frame.extend_from_slice(&crc.to_le_bytes());
    frame
}

/// COBS-encode (with trailing zero delimiter).
fn cobs_encode(raw: &[u8]) -> Vec<u8, MAX_FRAME_SIZE> {
    let mut out: Vec<u8, MAX_FRAME_SIZE> = Vec::new();
    let _ = out.resize(corncobs::max_encoded_len(raw.len()), 0);
    let len = corncobs::encode_buf(raw, &mut out);
    out.truncate(len);
    out
}

/// Validate a raw frame and return `(id, payload)`.
fn parse_raw(data: &[u8]) -> Result<(u8, &[u8]), ParseError> {
    if data.len() < 6 {
        return Err(ParseError::TooShort);
    }
    if data[0] != PROTOCOL_VERSION {
        return Err(ParseError::InvalidVersion);
    }
    let id = data[1];
    let length = u16::from_le_bytes([data[2], data[3]]) as usize;
    if data.len() < 4 + length + 2 {
        return Err(ParseError::TooShort);
    }
    let payload = &data[4..4 + length];
    let received = u16::from_le_bytes([data[4 + length], data[5 + length]]);
    if crc16(&data[..4 + length]) != received {
        return Err(ParseError::CrcError);
    }
    Ok((id, payload))
}

/// Sequential reader over a payload for length-prefixed fields.
struct PayloadReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> PayloadReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        if self.pos + n > self.data.len() {
            return Err(ParseError::TooShort);
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ParseError> {
        Ok(self.take(1)?[0])
    }

    fn u16_le(&mut self) -> Result<u16, ParseError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32_le(&mut self) -> Result<u32, ParseError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A one-byte-length-prefixed UTF-8 string capped at `N`.
    fn string<const N: usize>(&mut self) -> Result<String<N>, ParseError> {
        let len = self.u8()? as usize;
        if len > N {
            return Err(ParseError::BadPayload);
        }
        let bytes = self.take(len)?;
        let text = core::str::from_utf8(bytes).map_err(|_| ParseError::BadPayload)?;
        let mut out = String::new();
        out.push_str(text).map_err(|_| ParseError::BadPayload)?;
        Ok(out)
    }

    fn finished(&self) -> bool {
        self.pos == self.data.len()
    }
}

fn push_string(payload: &mut Vec<u8, MAX_FRAME_SIZE>, s: &str) {
    let _ = payload.push(s.len() as u8);
    let _ = payload.extend_from_slice(s.as_bytes());
}

fn encode_gateway_fields(payload: &mut Vec<u8, MAX_FRAME_SIZE>, gw: &GatewaySettings) {
    let _ = payload.extend_from_slice(&gw.frequency_hz.to_le_bytes());
    let _ = payload.push(gw.spreading_factor);
    let _ = payload.extend_from_slice(&gw.bandwidth_khz.to_le_bytes());
    let _ = payload.push(gw.coding_rate);
    let _ = payload.extend_from_slice(&gw.port.to_le_bytes());
    push_string(payload, &gw.host);
}

fn parse_gateway_fields(r: &mut PayloadReader) -> Result<GatewaySettings, ParseError> {
    Ok(GatewaySettings {
        frequency_hz: r.u32_le()?,
        spreading_factor: r.u8()?,
        bandwidth_khz: r.u32_le()?,
        coding_rate: r.u8()?,
        port: r.u16_le()?,
        host: r.string()?,
    })
}

// --- Host role: send commands, parse responses -------------------------------

/// Encode a hub command into a COBS frame ready for the wire.
pub fn encode_command(command: &HubCommand) -> Vec<u8, MAX_FRAME_SIZE> {
    let mut payload: Vec<u8, MAX_FRAME_SIZE> = Vec::new();
    match command {
        HubCommand::SetWifiConfig(wifi) => {
            push_string(&mut payload, &wifi.ssid);
            push_string(&mut payload, &wifi.password);
        }
        HubCommand::SetMqttConfig(mqtt) => {
            let _ = payload.extend_from_slice(&mqtt.port.to_le_bytes());
            push_string(&mut payload, &mqtt.host);
            push_string(&mut payload, &mqtt.client_id);
        }
        HubCommand::GetNetworkConfig
        | HubCommand::GetHubStatus
        | HubCommand::ClearNetworkConfig => {}
        HubCommand::SetMode { mode } => {
            let _ = payload.push(*mode as u8);
        }
        HubCommand::SetGatewayConfig(gw) => encode_gateway_fields(&mut payload, gw),
    }
    cobs_encode(&build_raw(command.id() as u8, &payload))
}

/// Parse a COBS-decoded frame into a hub response.
pub fn parse_response(data: &[u8]) -> Result<HubResponse, ParseError> {
    let (id, payload) = parse_raw(data)?;
    let mut r = PayloadReader::new(payload);
    let response = match HubResponseId::from_byte(id) {
        Some(HubResponseId::NetworkConfig) => {
            let mode = HubMode::from_byte(r.u8()?).ok_or(ParseError::BadPayload)?;
            let mqtt_port = r.u16_le()?;
            let gateway_prefix = parse_gateway_prefix(&mut r)?;
            let wifi_ssid = r.string()?;
            let mqtt_host = r.string()?;
            let client_id = r.string()?;
            let gw_host = r.string()?;
            HubResponse::NetworkConfig {
                mode,
                wifi_ssid,
                mqtt: MqttSettings { host: mqtt_host, port: mqtt_port, client_id },
                gateway: GatewaySettings {
                    frequency_hz: gateway_prefix.0,
                    spreading_factor: gateway_prefix.1,
                    bandwidth_khz: gateway_prefix.2,
                    coding_rate: gateway_prefix.3,
                    port: gateway_prefix.4,
                    host: gw_host,
                },
            }
        }
        Some(HubResponseId::HubStatus) => {
            let wifi_state = LinkState::from_byte(r.u8()?).ok_or(ParseError::BadPayload)?;
            let ip_bytes = r.take(4)?;
            let mqtt_state = LinkState::from_byte(r.u8()?).ok_or(ParseError::BadPayload)?;
            HubResponse::HubStatus(HubStatus {
                wifi_state,
                ip: [ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]],
                mqtt_state,
                uplink_count: r.u32_le()?,
                downlink_count: r.u32_le()?,
                dropped_count: r.u32_le()?,
                uptime_secs: r.u32_le()?,
            })
        }
        Some(HubResponseId::ConfigAck) => {
            let status = AckStatus::from_byte(r.u8()?).ok_or(ParseError::BadPayload)?;
            HubResponse::ConfigAck { status }
        }
        None => return Err(ParseError::UnknownId(id)),
    };
    if !r.finished() {
        return Err(ParseError::BadPayload);
    }
    Ok(response)
}

/// The fixed-width gateway fields as laid out in NetworkConfig
/// (freq, sf, bw, cr, port), which precede the variable-length strings.
fn parse_gateway_prefix(r: &mut PayloadReader) -> Result<(u32, u8, u32, u8, u16), ParseError> {
    Ok((r.u32_le()?, r.u8()?, r.u32_le()?, r.u8()?, r.u16_le()?))
}

// --- Device role: parse commands, send responses -----------------------------

/// Parse a COBS-decoded frame into a hub command. Errors are protocol status
/// codes so the device can reply with a matching `Response::Error`.
pub fn parse_command(data: &[u8]) -> Result<HubCommand, ResponseStatus> {
    let (id, payload) = match parse_raw(data) {
        Ok(v) => v,
        Err(ParseError::InvalidVersion) => return Err(ResponseStatus::InvalidVersion),
        Err(ParseError::CrcError) => return Err(ResponseStatus::CrcError),
        Err(_) => return Err(ResponseStatus::InvalidLength),
    };
    let id = match HubCommandId::from_byte(id) {
        Some(id) => id,
        None => return Err(ResponseStatus::InvalidCommand),
    };
    let mut r = PayloadReader::new(payload);
    let command = match id {
        HubCommandId::SetWifiConfig => {
            let ssid = r.string().map_err(|_| ResponseStatus::InvalidLength)?;
            let password = r.string().map_err(|_| ResponseStatus::InvalidLength)?;
            HubCommand::SetWifiConfig(WifiSettings { ssid, password })
        }
        HubCommandId::SetMqttConfig => {
            let port = r.u16_le().map_err(|_| ResponseStatus::InvalidLength)?;
            let host = r.string().map_err(|_| ResponseStatus::InvalidLength)?;
            let client_id = r.string().map_err(|_| ResponseStatus::InvalidLength)?;
            HubCommand::SetMqttConfig(MqttSettings { host, port, client_id })
        }
        HubCommandId::GetNetworkConfig => HubCommand::GetNetworkConfig,
        HubCommandId::GetHubStatus => HubCommand::GetHubStatus,
        HubCommandId::SetMode => {
            let byte = r.u8().map_err(|_| ResponseStatus::InvalidLength)?;
            let mode = HubMode::from_byte(byte).ok_or(ResponseStatus::InvalidParameter)?;
            HubCommand::SetMode { mode }
        }
        HubCommandId::ClearNetworkConfig => HubCommand::ClearNetworkConfig,
        HubCommandId::SetGatewayConfig => {
            let gw = parse_gateway_fields(&mut r).map_err(|_| ResponseStatus::InvalidLength)?;
            HubCommand::SetGatewayConfig(gw)
        }
    };
    if !r.finished() {
        return Err(ResponseStatus::InvalidLength);
    }
    Ok(command)
}

/// Encode a hub response into a COBS frame ready for the wire.
pub fn encode_response(response: &HubResponse) -> Vec<u8, MAX_FRAME_SIZE> {
    let mut payload: Vec<u8, MAX_FRAME_SIZE> = Vec::new();
    let id = match response {
        HubResponse::NetworkConfig { mode, wifi_ssid, mqtt, gateway } => {
            let _ = payload.push(*mode as u8);
            let _ = payload.extend_from_slice(&mqtt.port.to_le_bytes());
            let _ = payload.extend_from_slice(&gateway.frequency_hz.to_le_bytes());
            let _ = payload.push(gateway.spreading_factor);
            let _ = payload.extend_from_slice(&gateway.bandwidth_khz.to_le_bytes());
            let _ = payload.push(gateway.coding_rate);
            let _ = payload.extend_from_slice(&gateway.port.to_le_bytes());
            push_string(&mut payload, wifi_ssid);
            push_string(&mut payload, &mqtt.host);
            push_string(&mut payload, &mqtt.client_id);
            push_string(&mut payload, &gateway.host);
            HubResponseId::NetworkConfig
        }
        HubResponse::HubStatus(status) => {
            let _ = payload.push(status.wifi_state as u8);
            let _ = payload.extend_from_slice(&status.ip);
            let _ = payload.push(status.mqtt_state as u8);
            let _ = payload.extend_from_slice(&status.uplink_count.to_le_bytes());
            let _ = payload.extend_from_slice(&status.downlink_count.to_le_bytes());
            let _ = payload.extend_from_slice(&status.dropped_count.to_le_bytes());
            let _ = payload.extend_from_slice(&status.uptime_secs.to_le_bytes());
            HubResponseId::HubStatus
        }
        HubResponse::ConfigAck { status } => {
            let _ = payload.push(*status as u8);
            HubResponseId::ConfigAck
        }
    };
    cobs_encode(&build_raw(id as u8, &payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wt_protocol::cobs_decode;

    fn wifi() -> WifiSettings {
        WifiSettings {
            ssid: String::try_from("HomeNet").unwrap(),
            password: String::try_from("hunter22secret").unwrap(),
        }
    }

    fn mqtt() -> MqttSettings {
        MqttSettings {
            host: String::try_from("192.168.1.10").unwrap(),
            port: 1883,
            client_id: String::try_from("wt-hub-AABBCC").unwrap(),
        }
    }

    fn gateway() -> GatewaySettings {
        GatewaySettings {
            frequency_hz: 868_100_000,
            spreading_factor: 7,
            bandwidth_khz: 125,
            coding_rate: 5,
            host: String::try_from("chirpstack.local").unwrap(),
            port: 1700,
        }
    }

    #[test]
    fn commands_round_trip_device_side() {
        let commands = [
            HubCommand::SetWifiConfig(wifi()),
            HubCommand::SetMqttConfig(mqtt()),
            HubCommand::GetNetworkConfig,
            HubCommand::GetHubStatus,
            HubCommand::SetMode { mode: HubMode::LorawanGateway },
            HubCommand::ClearNetworkConfig,
            HubCommand::SetGatewayConfig(gateway()),
        ];
        for cmd in commands {
            let frame = encode_command(&cmd);
            let decoded = cobs_decode(&frame).unwrap();
            assert_eq!(parse_command(&decoded), Ok(cmd));
        }
    }

    #[test]
    fn responses_round_trip_host_side() {
        let responses = [
            HubResponse::NetworkConfig {
                mode: HubMode::Bridge,
                wifi_ssid: String::try_from("HomeNet").unwrap(),
                mqtt: mqtt(),
                gateway: gateway(),
            },
            HubResponse::HubStatus(HubStatus {
                wifi_state: LinkState::Connected,
                ip: [192, 168, 1, 42],
                mqtt_state: LinkState::Connecting,
                uplink_count: 17,
                downlink_count: 3,
                dropped_count: 1,
                uptime_secs: 3600,
            }),
            HubResponse::ConfigAck { status: AckStatus::Ok },
        ];
        for resp in responses {
            let frame = encode_response(&resp);
            let decoded = cobs_decode(&frame).unwrap();
            assert_eq!(parse_response(&decoded), Ok(resp));
        }
    }

    #[test]
    fn empty_strings_round_trip() {
        let cmd = HubCommand::SetWifiConfig(WifiSettings::default());
        let decoded = cobs_decode(&encode_command(&cmd)).unwrap();
        assert_eq!(parse_command(&decoded), Ok(cmd));
    }

    #[test]
    fn kat_set_mode_command() {
        // version 1, id 0x24, len 1, payload 0x01, then CRC and COBS. Layout
        // check to freeze the wire format.
        let frame = encode_command(&HubCommand::SetMode { mode: HubMode::LorawanGateway });
        let decoded = cobs_decode(&frame).unwrap();
        assert_eq!(&decoded[..5], &[0x01, 0x24, 0x01, 0x00, 0x01]);
        let crc = u16::from_le_bytes([decoded[5], decoded[6]]);
        assert_eq!(crc, crc16(&decoded[..5]));
    }

    #[test]
    fn wt_protocol_ids_are_not_hub_commands() {
        // A stock wt-protocol command must fall through to InvalidCommand so
        // the two-stage parser routes it to the right codec.
        let frame = wt_protocol::encode_command(&wt_protocol::Command::GetVersion);
        let decoded = cobs_decode(&frame).unwrap();
        assert_eq!(parse_command(&decoded), Err(ResponseStatus::InvalidCommand));
    }

    #[test]
    fn oversized_string_is_rejected() {
        // 33-byte SSID exceeds MAX_SSID_LEN; build the payload by hand.
        let mut payload: Vec<u8, { MAX_FRAME_SIZE }> = Vec::new();
        payload.push(33).unwrap();
        payload.extend_from_slice(&[b'a'; 33]).unwrap();
        payload.push(0).unwrap();
        let raw = build_raw(HubCommandId::SetWifiConfig as u8, &payload);
        assert_eq!(parse_command(&raw), Err(ResponseStatus::InvalidLength));
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut payload: Vec<u8, { MAX_FRAME_SIZE }> = Vec::new();
        payload.push(HubMode::Bridge as u8).unwrap();
        payload.push(0xAA).unwrap();
        let raw = build_raw(HubCommandId::SetMode as u8, &payload);
        assert_eq!(parse_command(&raw), Err(ResponseStatus::InvalidLength));
    }

    #[test]
    fn invalid_mode_is_invalid_parameter() {
        let raw = build_raw(HubCommandId::SetMode as u8, &[9]);
        assert_eq!(parse_command(&raw), Err(ResponseStatus::InvalidParameter));
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let mut payload: Vec<u8, { MAX_FRAME_SIZE }> = Vec::new();
        payload.push(2).unwrap();
        payload.extend_from_slice(&[0xFF, 0xFE]).unwrap();
        payload.push(0).unwrap();
        let raw = build_raw(HubCommandId::SetWifiConfig as u8, &payload);
        assert_eq!(parse_command(&raw), Err(ResponseStatus::InvalidLength));
    }

    #[test]
    fn corrupt_crc_is_rejected() {
        let frame = encode_command(&HubCommand::GetHubStatus);
        let mut raw = cobs_decode(&frame).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xFF;
        assert_eq!(parse_command(&raw), Err(ResponseStatus::CrcError));
        assert_eq!(parse_response(&raw), Err(ParseError::CrcError));
    }
}
