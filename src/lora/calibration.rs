//! SX1262 calibration parameters
//!
//! Dependency-free helpers shared by the driver and unit tests. Kept separate
//! from the driver so the band mapping can be tested on the host without the
//! embedded HAL stack.

/// Calibrate all blocks (RC64k, RC13M, PLL, ADC x3, image).
///
/// Used with the Calibrate (0x89) command. Required after enabling the TCXO,
/// because the power-on calibration ran from the RC oscillator before the
/// TCXO was available.
pub const CALIBRATE_ALL: u8 = 0x7F;

/// Image calibration band bytes for the CalibrateImage (0x98) command.
///
/// The SX126x only stores image calibration for one band at a time, so it must
/// be redone for the operating frequency. Bytes are taken from the datasheet
/// frequency table. Frequencies outside the listed bands fall back to the
/// 863-870 MHz (EU868) band used by this firmware.
pub fn image_cal_params(freq_hz: u32) -> (u8, u8) {
    match freq_hz {
        430_000_000..=440_000_000 => (0x6B, 0x6F),
        470_000_000..=510_000_000 => (0x75, 0x81),
        779_000_000..=787_000_000 => (0xC1, 0xC5),
        863_000_000..=870_000_000 => (0xD7, 0xDB),
        902_000_000..=928_000_000 => (0xE1, 0xE9),
        _ => (0xD7, 0xDB),
    }
}

/// Whether low data rate optimisation must be enabled.
///
/// The datasheet requires LDRO when the LoRa symbol time reaches 16.38 ms:
/// SF11 and SF12 at 125 kHz, and SF12 at 250 kHz. The symbol time is
/// 2^SF / BW; the bandwidth label in kHz is close enough for the threshold,
/// which only bites at 125/250 kHz.
pub fn ldro_enabled(spreading_factor: u8, bandwidth_khz: u32) -> bool {
    if bandwidth_khz == 0 || spreading_factor >= 32 {
        return true;
    }
    let symbol_time_us = (1u64 << spreading_factor) * 1000 / bandwidth_khz as u64;
    symbol_time_us >= 16_384
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_frequency_maps_to_eu868_band() {
        // 869.525 MHz is the firmware default and sits in the 863-870 MHz band.
        assert_eq!(image_cal_params(869_525_000), (0xD7, 0xDB));
    }

    #[test]
    fn each_band_maps_to_datasheet_bytes() {
        assert_eq!(image_cal_params(434_000_000), (0x6B, 0x6F));
        assert_eq!(image_cal_params(490_000_000), (0x75, 0x81));
        assert_eq!(image_cal_params(783_000_000), (0xC1, 0xC5));
        assert_eq!(image_cal_params(868_000_000), (0xD7, 0xDB));
        assert_eq!(image_cal_params(915_000_000), (0xE1, 0xE9));
    }

    #[test]
    fn band_edges_are_inclusive() {
        assert_eq!(image_cal_params(863_000_000), (0xD7, 0xDB));
        assert_eq!(image_cal_params(870_000_000), (0xD7, 0xDB));
    }

    #[test]
    fn out_of_band_falls_back_to_eu868() {
        assert_eq!(image_cal_params(100_000_000), (0xD7, 0xDB));
    }

    #[test]
    fn ldro_matches_datasheet_combinations() {
        // Datasheet 13.4.5: "typically SF11 with BW125 and SF12 with BW125
        // and BW250".
        assert!(ldro_enabled(11, 125));
        assert!(ldro_enabled(12, 125));
        assert!(ldro_enabled(12, 250));
        // The firmware default (SF11 at 250 kHz) sits below the threshold.
        assert!(!ldro_enabled(11, 250));
        assert!(!ldro_enabled(12, 500));
        assert!(!ldro_enabled(7, 125));
        // Very low bandwidths always need it.
        assert!(ldro_enabled(9, 8));
    }
}
