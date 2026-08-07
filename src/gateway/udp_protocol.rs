//! Semtech UDP packet-forwarder protocol (version 2).
//!
//! Datagram layout: `[version:0x02][token:u16][identifier]` followed by the
//! gateway EUI (upstream packets) and/or a JSON object. Uplinks travel as
//! `rxpk` JSON in PUSH_DATA; downlinks arrive as `txpk` JSON in PULL_RESP.
//! Byte-level facts verified against Semtech's PROTOCOL.TXT and ChirpStack's
//! semtech_udp backend.

use core::fmt::Write as _;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use heapless::Vec;

pub const PROTOCOL_VERSION: u8 = 2;

pub mod packet_id {
    pub const PUSH_DATA: u8 = 0x00;
    pub const PUSH_ACK: u8 = 0x01;
    pub const PULL_DATA: u8 = 0x02;
    pub const PULL_RESP: u8 = 0x03;
    pub const PULL_ACK: u8 = 0x04;
    pub const TX_ACK: u8 = 0x05;
}

/// Buffer sizes for callers: a PUSH_DATA with one max-size rxpk needs
/// 12 + JSON (~360 base64 chars + ~150 fixed) bytes.
pub const PUSH_BUF: usize = 600;
pub const ACK_BUF: usize = 64;

/// Metadata for one received uplink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RxMeta {
    /// Microsecond counter at RX completion (wraps every ~71.6 minutes).
    pub tmst: u32,
    pub freq_hz: u32,
    pub spreading_factor: u8,
    pub bandwidth_khz: u32,
    pub coding_rate: u8,
    pub rssi: i16,
    pub snr: i8,
}

/// Counters for the periodic `stat` message.
#[derive(Debug, Clone, Copy, Default)]
pub struct StatCounters {
    /// Radio packets received (CRC OK - the driver drops CRC failures).
    pub rxnb: u32,
    /// Packets received with a valid CRC.
    pub rxok: u32,
    /// Packets forwarded upstream.
    pub rxfw: u32,
    /// PUSH_DATA datagrams acknowledged, for the ack ratio.
    pub push_acked: u32,
    /// PUSH_DATA datagrams sent.
    pub push_sent: u32,
    /// Downlink datagrams received (PULL_RESP).
    pub dwnb: u32,
    /// Downlinks actually emitted by the radio.
    pub txnb: u32,
}

/// TX_ACK error values ChirpStack's gw.TxAckStatus enum recognises. There is
/// no TX_POWER variant here on purpose: over-limit power requests are
/// clamped, not refused (refusing would kill every 27 dBm RX2 request).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxAckError {
    None,
    TooLate,
    TooEarly,
    CollisionPacket,
    TxFreq,
    DutyCycleOverflow,
}

impl TxAckError {
    pub fn as_str(self) -> &'static str {
        match self {
            TxAckError::None => "NONE",
            TxAckError::TooLate => "TOO_LATE",
            TxAckError::TooEarly => "TOO_EARLY",
            TxAckError::CollisionPacket => "COLLISION_PACKET",
            TxAckError::TxFreq => "TX_FREQ",
            TxAckError::DutyCycleOverflow => "DUTY_CYCLE_OVERFLOW",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoError {
    /// Datagram too short, wrong version or unknown identifier.
    Malformed,
    /// The txpk JSON is missing fields or has values we cannot use.
    BadTxpk,
}

/// A downstream datagram from the network server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Downstream<'a> {
    PushAck { token: u16 },
    PullAck { token: u16 },
    /// The raw JSON payload of a PULL_RESP; parse with [`parse_txpk`].
    PullResp { token: u16, json: &'a [u8] },
}

/// A parsed and normalised txpk downlink request.
#[derive(Debug, Clone, PartialEq)]
pub struct TxPacket {
    pub immediate: bool,
    pub tmst: Option<u32>,
    pub freq_hz: u32,
    /// Requested power; the forwarder clamps it to the radio's ceiling.
    pub power_dbm: i8,
    pub spreading_factor: u8,
    pub bandwidth_khz: u32,
    pub coding_rate: u8,
    pub iq_inverted: bool,
    pub preamble: u16,
    pub payload: Vec<u8, 256>,
}

/// Writer that appends into a byte slice, tracking length.
struct SliceWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> SliceWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        let end = (self.len + bytes.len()).min(self.buf.len());
        let n = end - self.len;
        self.buf[self.len..end].copy_from_slice(&bytes[..n]);
        self.len = end;
    }
}

impl core::fmt::Write for SliceWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if self.len + s.len() > self.buf.len() {
            return Err(core::fmt::Error);
        }
        self.push_bytes(s.as_bytes());
        Ok(())
    }
}

fn header(writer: &mut SliceWriter, token: u16, id: u8, eui: Option<&[u8; 8]>) {
    writer.push_bytes(&[PROTOCOL_VERSION]);
    writer.push_bytes(&token.to_be_bytes());
    writer.push_bytes(&[id]);
    if let Some(eui) = eui {
        writer.push_bytes(eui);
    }
}

/// PUSH_DATA carrying one received packet. Returns the datagram length.
pub fn build_push_data_rxpk(
    out: &mut [u8],
    token: u16,
    eui: &[u8; 8],
    meta: &RxMeta,
    payload: &[u8],
) -> usize {
    let mut b64 = [0u8; 352];
    let b64_len = BASE64.encode_slice(payload, &mut b64).unwrap_or(0);
    let data = core::str::from_utf8(&b64[..b64_len]).unwrap_or("");

    let mut writer = SliceWriter::new(out);
    header(&mut writer, token, packet_id::PUSH_DATA, Some(eui));
    let _ = write!(
        writer,
        r#"{{"rxpk":[{{"tmst":{},"chan":0,"rfch":0,"freq":{}.{:06},"stat":1,"modu":"LORA","datr":"SF{}BW{}","codr":"4/{}","rssi":{},"lsnr":{},"size":{},"data":"{}"}}]}}"#,
        meta.tmst,
        meta.freq_hz / 1_000_000,
        meta.freq_hz % 1_000_000,
        meta.spreading_factor,
        meta.bandwidth_khz,
        meta.coding_rate,
        meta.rssi,
        meta.snr,
        payload.len(),
        data,
    );
    writer.len
}

/// PUSH_DATA carrying the periodic `stat` object.
pub fn build_push_data_stat(
    out: &mut [u8],
    token: u16,
    eui: &[u8; 8],
    stats: &StatCounters,
) -> usize {
    // Ack ratio as a percentage with one decimal, computed in tenths.
    let ackr_tenths = if stats.push_sent == 0 {
        1000
    } else {
        (stats.push_acked as u64 * 1000 / stats.push_sent as u64) as u32
    };
    let mut writer = SliceWriter::new(out);
    header(&mut writer, token, packet_id::PUSH_DATA, Some(eui));
    let _ = write!(
        writer,
        r#"{{"stat":{{"rxnb":{},"rxok":{},"rxfw":{},"ackr":{}.{},"dwnb":{},"txnb":{}}}}}"#,
        stats.rxnb,
        stats.rxok,
        stats.rxfw,
        ackr_tenths / 10,
        ackr_tenths % 10,
        stats.dwnb,
        stats.txnb,
    );
    writer.len
}

/// PULL_DATA keepalive (opens the NAT pinhole for downlinks).
pub fn build_pull_data(out: &mut [u8], token: u16, eui: &[u8; 8]) -> usize {
    let mut writer = SliceWriter::new(out);
    header(&mut writer, token, packet_id::PULL_DATA, Some(eui));
    writer.len
}

/// TX_ACK answering a PULL_RESP (token must echo the PULL_RESP's).
pub fn build_tx_ack(out: &mut [u8], token: u16, eui: &[u8; 8], error: TxAckError) -> usize {
    let mut writer = SliceWriter::new(out);
    header(&mut writer, token, packet_id::TX_ACK, Some(eui));
    let _ = write!(writer, r#"{{"txpk_ack":{{"error":"{}"}}}}"#, error.as_str());
    writer.len
}

/// Parse a downstream datagram from the server.
pub fn parse_datagram(data: &[u8]) -> Result<Downstream<'_>, ProtoError> {
    if data.len() < 4 || data[0] != PROTOCOL_VERSION {
        return Err(ProtoError::Malformed);
    }
    let token = u16::from_be_bytes([data[1], data[2]]);
    match data[3] {
        packet_id::PUSH_ACK => Ok(Downstream::PushAck { token }),
        packet_id::PULL_ACK => Ok(Downstream::PullAck { token }),
        packet_id::PULL_RESP => Ok(Downstream::PullResp { token, json: &data[4..] }),
        _ => Err(ProtoError::Malformed),
    }
}

#[derive(serde::Deserialize)]
struct TxpkWrapper<'a> {
    #[serde(borrow)]
    txpk: Txpk<'a>,
}

/// Raw txpk fields as ChirpStack sends them. `ncrc` and `prea` are never set
/// by ChirpStack; the gateway forces CRC off and preamble 8 for LoRa
/// downlinks itself.
#[derive(serde::Deserialize)]
struct Txpk<'a> {
    #[serde(default)]
    imme: bool,
    tmst: Option<u32>,
    freq: f64,
    powe: Option<i8>,
    modu: &'a str,
    datr: &'a str,
    codr: Option<&'a str>,
    #[serde(default)]
    ipol: bool,
    prea: Option<u16>,
    data: &'a str,
}

/// Parse and normalise the txpk JSON from a PULL_RESP.
pub fn parse_txpk(json: &[u8]) -> Result<TxPacket, ProtoError> {
    let (wrapper, _) =
        serde_json_core::from_slice::<TxpkWrapper>(json).map_err(|_| ProtoError::BadTxpk)?;
    let txpk = wrapper.txpk;

    if txpk.modu != "LORA" {
        return Err(ProtoError::BadTxpk);
    }
    let (spreading_factor, bandwidth_khz) = parse_datr(txpk.datr).ok_or(ProtoError::BadTxpk)?;
    // Tolerate "4/5LI" style suffixes: take the digit after the slash.
    let coding_rate = txpk
        .codr
        .and_then(|c| c.as_bytes().get(2).copied())
        .map(|b| b.wrapping_sub(b'0'))
        .filter(|cr| (5..=8).contains(cr))
        .unwrap_or(5);

    let mut payload = Vec::new();
    payload.resize_default(256).map_err(|_| ProtoError::BadTxpk)?;
    let len = BASE64
        .decode_slice(txpk.data.as_bytes(), &mut payload)
        .map_err(|_| ProtoError::BadTxpk)?;
    payload.truncate(len);
    if payload.is_empty() {
        return Err(ProtoError::BadTxpk);
    }

    Ok(TxPacket {
        immediate: txpk.imme,
        tmst: txpk.tmst,
        freq_hz: (txpk.freq * 1_000_000.0 + 0.5) as u32,
        power_dbm: txpk.powe.unwrap_or(14),
        spreading_factor,
        bandwidth_khz,
        coding_rate,
        iq_inverted: txpk.ipol,
        preamble: txpk.prea.unwrap_or(8),
        payload,
    })
}

/// "SF7BW125" -> (7, 125)
fn parse_datr(datr: &str) -> Option<(u8, u32)> {
    let rest = datr.strip_prefix("SF")?;
    let bw_at = rest.find("BW")?;
    let sf: u8 = rest[..bw_at].parse().ok()?;
    let bw: u32 = rest[bw_at + 2..].parse().ok()?;
    if !(5..=12).contains(&sf) {
        return None;
    }
    Some((sf, bw))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EUI: [u8; 8] = [0x24, 0x6F, 0x28, 0xFF, 0xFE, 0xAA, 0xBB, 0xCC];

    fn meta() -> RxMeta {
        RxMeta {
            tmst: 3_512_348_611,
            freq_hz: 868_100_000,
            spreading_factor: 7,
            bandwidth_khz: 125,
            coding_rate: 5,
            rssi: -87,
            snr: 5,
        }
    }

    #[test]
    fn push_data_layout_and_json() {
        let mut out = [0u8; PUSH_BUF];
        let len = build_push_data_rxpk(&mut out, 0x1A2B, &EUI, &meta(), b"Hi");
        assert_eq!(out[0], 0x02);
        assert_eq!(&out[1..3], &[0x1A, 0x2B]);
        assert_eq!(out[3], packet_id::PUSH_DATA);
        assert_eq!(&out[4..12], &EUI);
        let json = core::str::from_utf8(&out[12..len]).unwrap();
        assert_eq!(
            json,
            r#"{"rxpk":[{"tmst":3512348611,"chan":0,"rfch":0,"freq":868.100000,"stat":1,"modu":"LORA","datr":"SF7BW125","codr":"4/5","rssi":-87,"lsnr":5,"size":2,"data":"SGk="}]}"#
        );
    }

    #[test]
    fn push_data_fits_max_payload() {
        let mut out = [0u8; PUSH_BUF];
        let payload = [0xAA; 255];
        let len = build_push_data_rxpk(&mut out, 1, &EUI, &meta(), &payload);
        assert!(len > 12 && len <= PUSH_BUF);
        // The JSON must be complete (ends with the array + object close).
        assert!(core::str::from_utf8(&out[12..len]).unwrap().ends_with("\"}]}"));
    }

    #[test]
    fn stat_message_reports_counters_and_ack_ratio() {
        let mut out = [0u8; 256];
        let stats = StatCounters {
            rxnb: 10,
            rxok: 10,
            rxfw: 9,
            push_acked: 9,
            push_sent: 10,
            dwnb: 2,
            txnb: 2,
        };
        let len = build_push_data_stat(&mut out, 7, &EUI, &stats);
        let json = core::str::from_utf8(&out[12..len]).unwrap();
        assert_eq!(
            json,
            r#"{"stat":{"rxnb":10,"rxok":10,"rxfw":9,"ackr":90.0,"dwnb":2,"txnb":2}}"#
        );
    }

    #[test]
    fn pull_data_is_twelve_bytes() {
        let mut out = [0u8; 12];
        let len = build_pull_data(&mut out, 0xBEEF, &EUI);
        assert_eq!(len, 12);
        assert_eq!(out[3], packet_id::PULL_DATA);
    }

    #[test]
    fn tx_ack_encodes_the_error() {
        let mut out = [0u8; ACK_BUF];
        let len = build_tx_ack(&mut out, 3, &EUI, TxAckError::DutyCycleOverflow);
        let json = core::str::from_utf8(&out[12..len]).unwrap();
        assert_eq!(json, r#"{"txpk_ack":{"error":"DUTY_CYCLE_OVERFLOW"}}"#);
    }

    #[test]
    fn parses_acks_and_pull_resp() {
        assert_eq!(
            parse_datagram(&[0x02, 0x00, 0x07, 0x01]),
            Ok(Downstream::PushAck { token: 7 })
        );
        assert_eq!(
            parse_datagram(&[0x02, 0x12, 0x34, 0x04]),
            Ok(Downstream::PullAck { token: 0x1234 })
        );
        let resp = [&[0x02, 0x00, 0x01, 0x03][..], br#"{"txpk":{}}"#].concat();
        assert_eq!(
            parse_datagram(&resp),
            Ok(Downstream::PullResp { token: 1, json: br#"{"txpk":{}}"# })
        );
        assert_eq!(parse_datagram(&[0x01, 0, 0, 0]), Err(ProtoError::Malformed));
        assert_eq!(parse_datagram(&[0x02, 0, 0]), Err(ProtoError::Malformed));
        assert_eq!(parse_datagram(&[0x02, 0, 0, 0x09]), Err(ProtoError::Malformed));
    }

    #[test]
    fn parses_a_chirpstack_style_rx1_downlink() {
        // Shape matches ChirpStack's semtech_udp packets/pull_resp.go output;
        // the 28-char base64 payload decodes to 19 bytes.
        let json = br#"{"txpk":{"imme":false,"tmst":3513348611,"freq":868.1,"rfch":0,"powe":14,"modu":"LORA","datr":"SF7BW125","codr":"4/5","ipol":true,"size":19,"data":"YCkuASqAAAAByFaF53Iu+vzmwQ=="}}"#;
        let txpk = parse_txpk(json).unwrap();
        assert!(!txpk.immediate);
        assert_eq!(txpk.tmst, Some(3_513_348_611));
        assert_eq!(txpk.freq_hz, 868_100_000);
        assert_eq!(txpk.power_dbm, 14);
        assert_eq!(txpk.spreading_factor, 7);
        assert_eq!(txpk.bandwidth_khz, 125);
        assert_eq!(txpk.coding_rate, 5);
        assert!(txpk.iq_inverted);
        assert_eq!(txpk.preamble, 8);
        assert_eq!(txpk.payload.len(), 19);
    }

    #[test]
    fn parses_immediate_class_c_and_li_codr() {
        let json = br#"{"txpk":{"imme":true,"freq":869.525,"modu":"LORA","datr":"SF12BW125","codr":"4/5LI","ipol":true,"data":"AQI="}}"#;
        let txpk = parse_txpk(json).unwrap();
        assert!(txpk.immediate);
        assert_eq!(txpk.tmst, None);
        assert_eq!(txpk.freq_hz, 869_525_000);
        assert_eq!(txpk.spreading_factor, 12);
        assert_eq!(txpk.coding_rate, 5);
        assert_eq!(txpk.payload.as_slice(), &[0x01, 0x02]);
    }

    #[test]
    fn rejects_fsk_bad_datr_and_bad_base64() {
        let fsk = br#"{"txpk":{"freq":868.1,"modu":"FSK","datr":"50000","data":"AQI="}}"#;
        assert_eq!(parse_txpk(fsk), Err(ProtoError::BadTxpk));

        let bad_datr = br#"{"txpk":{"freq":868.1,"modu":"LORA","datr":"SF99BW125","data":"AQI="}}"#;
        assert_eq!(parse_txpk(bad_datr), Err(ProtoError::BadTxpk));

        let bad_b64 = br#"{"txpk":{"freq":868.1,"modu":"LORA","datr":"SF7BW125","data":"!!!"}}"#;
        assert_eq!(parse_txpk(bad_b64), Err(ProtoError::BadTxpk));
    }
}
