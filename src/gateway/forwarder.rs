//! Packet-forwarder state machine: tokens, counters, acks and the downlink
//! acceptance policy. Pure (all clocks injected) and host-tested; the UDP
//! task is a thin shell around it.

use heapless::{LinearMap, Vec};

use crate::lora::airtime::time_on_air_ms_ext;
use crate::lora::duty_cycle::{eu_band_key, DutyCycleLimiter};
use crate::lora::traits::{LoraConfig, SyncWord};

use super::scheduler::{classify, Classify};
use super::udp_protocol::{
    build_pull_data, build_push_data_rxpk, build_push_data_stat, build_tx_ack, parse_datagram,
    parse_txpk, Downstream, RxMeta, StatCounters, TxAckError, TxPacket,
};
use super::DownlinkJob;

/// The radio's TX power ceiling; server requests above it are clamped, not
/// refused (erroring would kill every 27 dBm RX2 downlink request).
const MAX_TX_POWER_DBM: i8 = 22;

/// EU sub-bands tracked for downlink duty cycle (RX1 band, RX2 band, spares).
const MAX_BANDS: usize = 4;

/// What the UDP task should do with a received datagram.
#[derive(Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // job moves straight into an embassy channel
pub enum DatagramOutcome {
    /// Nothing to send back (ack consumed, or unusable input).
    None,
    /// A rejection TX_ACK was written into `ack_out`; send these bytes.
    Reply(usize),
    /// The downlink passed policy. Queue the job, then send the TX_ACK built
    /// with [`GatewayForwarder::tx_ack`] (NONE if queued, COLLISION_PACKET
    /// if the queue was full).
    Accepted { job: DownlinkJob, token: u16 },
}

pub struct GatewayForwarder {
    eui: [u8; 8],
    token: u16,
    stats: StatCounters,
    duty: LinearMap<u8, DutyCycleLimiter, MAX_BANDS>,
}

impl GatewayForwarder {
    pub fn new(mac: &[u8; 6]) -> Self {
        Self {
            eui: super::eui::gateway_eui(mac),
            // Seed the token from the MAC so gateways do not collide.
            token: u16::from_be_bytes([mac[4], mac[5]]),
            stats: StatCounters::default(),
            duty: LinearMap::new(),
        }
    }

    pub fn stats(&self) -> &StatCounters {
        &self.stats
    }

    fn next_token(&mut self) -> u16 {
        self.token = self.token.wrapping_add(1);
        self.token
    }

    /// A received uplink becomes a PUSH_DATA datagram in `out`.
    pub fn on_uplink(&mut self, out: &mut [u8], meta: &RxMeta, payload: &[u8]) -> usize {
        self.stats.rxnb += 1;
        self.stats.rxok += 1;
        self.stats.rxfw += 1;
        self.stats.push_sent += 1;
        let token = self.next_token();
        build_push_data_rxpk(out, token, &self.eui, meta, payload)
    }

    /// The PULL_DATA keepalive; send every ~10 s to hold the downlink path.
    pub fn on_keepalive(&mut self, out: &mut [u8]) -> usize {
        let token = self.next_token();
        build_pull_data(out, token, &self.eui)
    }

    /// The periodic stat PUSH_DATA; send every ~30 s.
    pub fn on_stat(&mut self, out: &mut [u8]) -> usize {
        self.stats.push_sent += 1;
        let token = self.next_token();
        build_push_data_stat(out, token, &self.eui, &self.stats)
    }

    /// Build a TX_ACK for a previously accepted downlink.
    pub fn tx_ack(&mut self, out: &mut [u8], token: u16, error: TxAckError) -> usize {
        if error == TxAckError::None {
            // Counted at queue time; the radio task fires it unconditionally
            // unless its window has already passed.
            self.stats.txnb += 1;
        }
        build_tx_ack(out, token, &self.eui, error)
    }

    /// Process one datagram from the server. `now_us` is the tmst counter,
    /// `now_ms` feeds the duty cycle budget.
    pub fn on_datagram(
        &mut self,
        now_us: u32,
        now_ms: u64,
        data: &[u8],
        ack_out: &mut [u8],
    ) -> DatagramOutcome {
        let downstream = match parse_datagram(data) {
            Ok(d) => d,
            Err(_) => return DatagramOutcome::None,
        };
        match downstream {
            Downstream::PushAck { .. } => {
                self.stats.push_acked += 1;
                DatagramOutcome::None
            }
            Downstream::PullAck { .. } => DatagramOutcome::None,
            Downstream::PullResp { token, json } => {
                self.stats.dwnb += 1;
                let txpk = match parse_txpk(json) {
                    Ok(t) => t,
                    // Unusable request: nothing sensible to ack.
                    Err(_) => return DatagramOutcome::None,
                };
                match self.accept(now_us, now_ms, &txpk) {
                    Ok(job) => DatagramOutcome::Accepted { job, token },
                    Err(error) => {
                        DatagramOutcome::Reply(build_tx_ack(ack_out, token, &self.eui, error))
                    }
                }
            }
        }
    }

    /// The downlink acceptance policy.
    fn accept(
        &mut self,
        now_us: u32,
        now_ms: u64,
        txpk: &TxPacket,
    ) -> Result<DownlinkJob, TxAckError> {
        if !(863_000_000..=870_000_000).contains(&txpk.freq_hz) {
            return Err(TxAckError::TxFreq);
        }

        let target_tmst = match classify(now_us, txpk.immediate, txpk.tmst) {
            Classify::Immediate => None,
            Classify::At { .. } => txpk.tmst,
            Classify::TooLate => return Err(TxAckError::TooLate),
            Classify::TooEarly => return Err(TxAckError::TooEarly),
        };

        let airtime_ms = time_on_air_ms_ext(
            txpk.spreading_factor,
            txpk.bandwidth_khz,
            txpk.coding_rate,
            txpk.payload.len(),
            txpk.preamble,
            false,
        );
        let band = eu_band_key(txpk.freq_hz);
        if !self.duty.contains_key(&band) {
            let _ = self
                .duty
                .insert(band, DutyCycleLimiter::for_frequency(txpk.freq_hz));
        }
        if let Some(limiter) = self.duty.get_mut(&band) {
            limiter
                .try_claim(now_ms, airtime_ms)
                .map_err(|_| TxAckError::DutyCycleOverflow)?;
        }

        let mut payload = Vec::new();
        let _ = payload.extend_from_slice(&txpk.payload);
        Ok(DownlinkJob {
            target_tmst,
            config: LoraConfig {
                frequency_hz: txpk.freq_hz,
                spreading_factor: txpk.spreading_factor,
                bandwidth_khz: txpk.bandwidth_khz,
                coding_rate: txpk.coding_rate,
                tx_power_dbm: txpk.power_dbm.min(MAX_TX_POWER_DBM),
                sync_word: SyncWord::Public,
                iq_inverted: txpk.iq_inverted,
                // LoRa downlinks always go out without a payload CRC;
                // ChirpStack never sends ncrc and relies on this.
                crc_on: false,
                preamble_len: txpk.preamble,
            },
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::udp_protocol::{packet_id, ACK_BUF, PROTOCOL_VERSION};

    const MAC: [u8; 6] = [0x24, 0x6F, 0x28, 0xAA, 0xBB, 0xCC];

    fn pull_resp(token: u16, json: &str) -> std::vec::Vec<u8> {
        let mut data = vec![PROTOCOL_VERSION, (token >> 8) as u8, token as u8, packet_id::PULL_RESP];
        data.extend_from_slice(json.as_bytes());
        data
    }

    fn rx1_json(tmst: u32) -> std::string::String {
        format!(
            r#"{{"txpk":{{"imme":false,"tmst":{tmst},"freq":868.1,"powe":14,"modu":"LORA","datr":"SF7BW125","codr":"4/5","ipol":true,"data":"YCkuASqAAAAByFaF53Iu+vzmwQ=="}}}}"#
        )
    }

    #[test]
    fn accepted_rx1_downlink_builds_a_job() {
        let mut fwd = GatewayForwarder::new(&MAC);
        let mut ack = [0u8; ACK_BUF];
        let now_us = 1_000_000;
        let datagram = pull_resp(42, &rx1_json(now_us + 1_000_000));

        let outcome = fwd.on_datagram(now_us, 60_000, &datagram, &mut ack);
        let DatagramOutcome::Accepted { job, token } = outcome else {
            panic!("expected Accepted, got {outcome:?}");
        };
        assert_eq!(token, 42);
        assert_eq!(job.target_tmst, Some(now_us + 1_000_000));
        assert_eq!(job.config.frequency_hz, 868_100_000);
        assert_eq!(job.config.sync_word, SyncWord::Public);
        assert!(job.config.iq_inverted);
        assert!(!job.config.crc_on);
        assert_eq!(job.payload.len(), 19);

        // The success TX_ACK counts the downlink.
        let len = fwd.tx_ack(&mut ack, token, TxAckError::None);
        assert!(core::str::from_utf8(&ack[12..len]).unwrap().contains("NONE"));
        assert_eq!(fwd.stats().txnb, 1);
        assert_eq!(fwd.stats().dwnb, 1);
    }

    #[test]
    fn late_downlink_is_refused_too_late() {
        let mut fwd = GatewayForwarder::new(&MAC);
        let mut ack = [0u8; ACK_BUF];
        let now_us = 5_000_000;
        let datagram = pull_resp(1, &rx1_json(now_us - 1));

        let outcome = fwd.on_datagram(now_us, 60_000, &datagram, &mut ack);
        let DatagramOutcome::Reply(len) = outcome else {
            panic!("expected Reply, got {outcome:?}");
        };
        assert!(core::str::from_utf8(&ack[12..len]).unwrap().contains("TOO_LATE"));
    }

    #[test]
    fn out_of_band_frequency_is_refused() {
        let mut fwd = GatewayForwarder::new(&MAC);
        let mut ack = [0u8; ACK_BUF];
        let json = r#"{"txpk":{"imme":true,"freq":915.0,"modu":"LORA","datr":"SF7BW125","ipol":true,"data":"AQI="}}"#;
        let datagram = pull_resp(2, json);

        let outcome = fwd.on_datagram(0, 0, &datagram, &mut ack);
        let DatagramOutcome::Reply(len) = outcome else {
            panic!("expected Reply, got {outcome:?}");
        };
        assert!(core::str::from_utf8(&ack[12..len]).unwrap().contains("TX_FREQ"));
    }

    #[test]
    fn over_power_is_clamped_not_refused() {
        let mut fwd = GatewayForwarder::new(&MAC);
        let mut ack = [0u8; ACK_BUF];
        let json = r#"{"txpk":{"imme":true,"freq":869.525,"powe":27,"modu":"LORA","datr":"SF12BW125","ipol":true,"data":"AQI="}}"#;
        let datagram = pull_resp(3, json);

        let outcome = fwd.on_datagram(0, 0, &datagram, &mut ack);
        let DatagramOutcome::Accepted { job, .. } = outcome else {
            panic!("expected Accepted, got {outcome:?}");
        };
        assert_eq!(job.config.tx_power_dbm, MAX_TX_POWER_DBM);
    }

    #[test]
    fn duty_cycle_exhaustion_returns_the_chirpstack_error() {
        let mut fwd = GatewayForwarder::new(&MAC);
        let mut ack = [0u8; ACK_BUF];
        // 868.1 MHz sits in the 1% sub-band: 36 s of airtime per hour.
        // SF12/BW125 max payloads (~4 s each) exhaust it after a handful.
        let json = r#"{"txpk":{"imme":true,"freq":868.1,"powe":14,"modu":"LORA","datr":"SF12BW125","codr":"4/5","ipol":true,"data":"YCkuASqAAAAByFaF53Iu+vzmwQ=="}}"#;

        let mut accepted = 0;
        for i in 0..40 {
            let datagram = pull_resp(i, json);
            match fwd.on_datagram(0, 0, &datagram, &mut ack) {
                DatagramOutcome::Accepted { .. } => accepted += 1,
                DatagramOutcome::Reply(len) => {
                    let text = core::str::from_utf8(&ack[12..len]).unwrap();
                    assert!(text.contains("DUTY_CYCLE_OVERFLOW"), "got {text}");
                    assert!(accepted > 0, "at least one downlink must fit the budget");
                    return;
                }
                DatagramOutcome::None => panic!("datagram must parse"),
            }
        }
        panic!("duty cycle never triggered");
    }

    #[test]
    fn rx1_and_rx2_budgets_are_independent() {
        let mut fwd = GatewayForwarder::new(&MAC);
        let mut ack = [0u8; ACK_BUF];
        let rx1 = r#"{"txpk":{"imme":true,"freq":868.1,"modu":"LORA","datr":"SF12BW125","ipol":true,"data":"YCkuASqAAAAByFaF53Iu+vzmwQ=="}}"#;
        let rx2 = r#"{"txpk":{"imme":true,"freq":869.525,"modu":"LORA","datr":"SF12BW125","ipol":true,"data":"YCkuASqAAAAByFaF53Iu+vzmwQ=="}}"#;

        // Exhaust the 1% RX1 band.
        let mut token = 0u16;
        loop {
            token += 1;
            let datagram = pull_resp(token, rx1);
            if matches!(
                fwd.on_datagram(0, 0, &datagram, &mut ack),
                DatagramOutcome::Reply(_)
            ) {
                break;
            }
            assert!(token < 100, "RX1 budget never exhausted");
        }

        // RX2 sits in the 10% sub-band and must still accept.
        let datagram = pull_resp(200, rx2);
        assert!(matches!(
            fwd.on_datagram(0, 0, &datagram, &mut ack),
            DatagramOutcome::Accepted { .. }
        ));
    }

    #[test]
    fn push_ack_improves_the_ack_ratio() {
        let mut fwd = GatewayForwarder::new(&MAC);
        let mut out = [0u8; crate::gateway::udp_protocol::PUSH_BUF];
        let meta = RxMeta {
            tmst: 1,
            freq_hz: 868_100_000,
            spreading_factor: 7,
            bandwidth_khz: 125,
            coding_rate: 5,
            rssi: -80,
            snr: 6,
        };
        let len = fwd.on_uplink(&mut out, &meta, b"x");
        assert!(len > 12);
        assert_eq!(fwd.stats().push_sent, 1);
        assert_eq!(fwd.stats().push_acked, 0);

        // Ack it with the token that was used.
        let token = u16::from_be_bytes([out[1], out[2]]);
        let ack_datagram = [PROTOCOL_VERSION, (token >> 8) as u8, token as u8, packet_id::PUSH_ACK];
        let mut ack = [0u8; ACK_BUF];
        assert_eq!(
            fwd.on_datagram(0, 0, &ack_datagram, &mut ack),
            DatagramOutcome::None
        );
        assert_eq!(fwd.stats().push_acked, 1);
    }
}
