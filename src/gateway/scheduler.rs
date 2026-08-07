//! Downlink timing decisions over the wrapping 32-bit microsecond counter.
//!
//! `tmst` wraps every 2^32 us (~71.6 minutes); all arithmetic is wrapping,
//! giving a +/-35.8 minute comparison horizon - far beyond the 30 second
//! acceptance window.

/// Time needed ahead of the target to reconfigure the radio and stage the
/// payload (~3 ms of SPI at 1 MHz plus executor jitter headroom).
pub const SETUP_LEAD_US: u32 = 20_000;

/// Compensation for the SetTx command, standby-to-TX transition and PA ramp;
/// the radio is told to fire this early so RF starts on the target.
pub const TX_START_COMP_US: u32 = 450;

/// Downlinks further out than this are refused as TOO_EARLY.
pub const MAX_AHEAD_US: i32 = 30_000_000;

/// Signed microseconds from `now` to `target` in wrapping u32 space.
pub fn tmst_delta_us(now: u32, target: u32) -> i32 {
    target.wrapping_sub(now) as i32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classify {
    /// Transmit now (Class C immediate).
    Immediate,
    /// Sleep until roughly `delta_us - SETUP_LEAD_US` from now, then stage
    /// and fire on the target.
    At { delta_us: u32 },
    TooLate,
    TooEarly,
}

/// Decide how to schedule a downlink given the current counter value.
pub fn classify(now: u32, immediate: bool, target_tmst: Option<u32>) -> Classify {
    if immediate {
        return Classify::Immediate;
    }
    let Some(target) = target_tmst else {
        // Neither immediate nor a target time: nothing to schedule.
        return Classify::TooLate;
    };
    let delta = tmst_delta_us(now, target);
    if delta < SETUP_LEAD_US as i32 {
        return Classify::TooLate;
    }
    if delta > MAX_AHEAD_US {
        return Classify::TooEarly;
    }
    Classify::At { delta_us: delta as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_handles_wraparound() {
        // Target just after the counter wraps, now just before.
        assert_eq!(tmst_delta_us(u32::MAX - 999, 1_000), 2_000);
        // Target in the past across the wrap boundary.
        assert_eq!(tmst_delta_us(1_000, u32::MAX - 999), -2_000);
        assert_eq!(tmst_delta_us(5_000, 5_000), 0);
    }

    #[test]
    fn classify_boundaries() {
        let now = 1_000_000u32;
        assert_eq!(classify(now, true, None), Classify::Immediate);
        assert_eq!(classify(now, false, None), Classify::TooLate);
        // Exactly one RX1 second ahead.
        assert_eq!(
            classify(now, false, Some(now + 1_000_000)),
            Classify::At { delta_us: 1_000_000 }
        );
        // Inside the setup lead: unachievable.
        assert_eq!(
            classify(now, false, Some(now + SETUP_LEAD_US - 1)),
            Classify::TooLate
        );
        // In the past.
        assert_eq!(classify(now, false, Some(now - 1)), Classify::TooLate);
        // Beyond the acceptance window.
        assert_eq!(
            classify(now, false, Some(now + MAX_AHEAD_US as u32 + 1)),
            Classify::TooEarly
        );
    }

    #[test]
    fn classify_across_the_wrap() {
        let now = u32::MAX - 100_000;
        assert_eq!(
            classify(now, false, Some(now.wrapping_add(2_000_000))),
            Classify::At { delta_us: 2_000_000 }
        );
    }
}
