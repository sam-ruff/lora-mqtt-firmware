//! LoRa time-on-air calculation (Semtech SX126x datasheet, section 6.1.4).
//!
//! Dependency-free so the duty cycle maths can be unit-tested on the host.
//! Assumes the packet parameters this firmware always uses: 8-symbol preamble,
//! explicit header and CRC enabled (see `Sx1262Driver::set_packet_params`).

use crate::lora::calibration::ldro_enabled;

/// Preamble length in symbols, matching the driver's packet params.
const PREAMBLE_SYMBOLS: u32 = 8;

/// Map the bandwidth label in kHz to the exact bandwidth in Hz.
///
/// Same label set (and fallback) as the driver's SetModulationParams mapping,
/// so the airtime calculation always agrees with what is on the air.
fn bandwidth_hz(bandwidth_khz: u32) -> u32 {
    match bandwidth_khz {
        7 | 8 => 7_800,
        10 => 10_400,
        15 | 16 => 15_600,
        20 | 21 => 20_800,
        31 => 31_250,
        41 | 42 => 41_700,
        62 | 63 => 62_500,
        125 => 125_000,
        250 => 250_000,
        500 => 500_000,
        _ => 125_000,
    }
}

/// Time on air in milliseconds (rounded up) for one LoRa packet.
///
/// `coding_rate` is the denominator (5-8 for 4/5 to 4/8); out-of-range values
/// fall back to 4/5, matching the driver. Spreading factors are clamped to the
/// 7-12 range this firmware permits.
pub fn time_on_air_ms(
    spreading_factor: u8,
    bandwidth_khz: u32,
    coding_rate: u8,
    payload_len: usize,
) -> u32 {
    let sf = spreading_factor.clamp(7, 12) as u32;
    let cr = match coding_rate {
        5..=8 => (coding_rate - 4) as u32,
        _ => 1,
    };
    let de = if ldro_enabled(sf as u8, bandwidth_khz) { 1u32 } else { 0 };

    // Payload symbol count: 8 + max(ceil((8*PL - 4*SF + 28 + 16) / (4*(SF - 2*DE))) * (CR + 4), 0)
    // with explicit header (IH = 0) and CRC on (+16).
    let numerator = 8 * payload_len as i64 - 4 * sf as i64 + 28 + 16;
    let denominator = (4 * (sf - 2 * de)) as i64;
    let blocks = if numerator > 0 {
        (numerator + denominator - 1) / denominator
    } else {
        0
    };
    let payload_symbols = 8 + blocks as u32 * (cr + 4);

    // Total in quarter-symbols: the preamble adds 4.25 sync symbols.
    let quarter_symbols = (PREAMBLE_SYMBOLS + payload_symbols) as u64 * 4 + 17;
    let symbol_time_us = ((1u64 << sf) * 1_000_000) / bandwidth_hz(bandwidth_khz) as u64;
    let total_us = quarter_symbols * symbol_time_us / 4;

    total_us.div_ceil(1000) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_known_sf7_value() {
        // Canonical LoRa calculator value: SF7, BW125, CR4/5, 13-byte payload,
        // 8-symbol preamble, explicit header, CRC on = 46.336 ms.
        assert_eq!(time_on_air_ms(7, 125, 5, 13), 47);
    }

    #[test]
    fn matches_known_sf12_value() {
        // SF12, BW250, CR4/8, 20 bytes, LDRO on: 52.25 symbols of 16.384 ms
        // = 856.064 ms.
        assert_eq!(time_on_air_ms(12, 250, 8, 20), 857);
    }

    #[test]
    fn max_payload_at_sf12_is_about_seven_seconds() {
        let ms = time_on_air_ms(12, 250, 8, 255);
        assert!((6_900..7_100).contains(&ms), "got {ms} ms");
    }

    #[test]
    fn max_payload_at_sf11_default_is_about_three_seconds() {
        // 396.25 symbols of 8.192 ms at SF11/BW250/CR4/8, no LDRO.
        assert_eq!(time_on_air_ms(11, 250, 8, 255), 3_247);
    }

    #[test]
    fn airtime_grows_with_spreading_factor() {
        let mut previous = 0;
        for sf in 7..=12 {
            let ms = time_on_air_ms(sf, 250, 8, 100);
            assert!(ms > previous, "SF{sf} not slower than SF{}", sf - 1);
            previous = ms;
        }
    }

    #[test]
    fn empty_payload_still_costs_preamble_and_header() {
        // The header and CRC alone need one coded block: 12.25 preamble +
        // 13 payload symbols at SF7/BW250 (0.512 ms/symbol) = 12.928 ms.
        assert_eq!(time_on_air_ms(7, 250, 5, 0), 13);
    }

    #[test]
    fn out_of_range_inputs_fall_back_safely() {
        // SF clamps into 7-12, coding rate falls back to 4/5: both resolve to
        // valid airtime rather than panicking.
        assert_eq!(time_on_air_ms(0, 250, 0, 10), time_on_air_ms(7, 250, 5, 10));
        assert_eq!(time_on_air_ms(200, 250, 8, 10), time_on_air_ms(12, 250, 8, 10));
    }
}
