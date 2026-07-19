//! EU 868 MHz duty cycle enforcement (ERC Recommendation 70-03 / EN 300 220).
//!
//! Duty cycle is defined per sub-band as the maximum cumulative transmitter
//! on-time over any one hour period. The limiter tracks spent airtime in
//! one-minute buckets covering a sliding hour, which is O(1) memory and errs
//! on the conservative side: a bucket only stops counting once the whole
//! minute has left the window.
//!
//! Dependency-free so the whole policy can be unit-tested on the host.

/// One-minute buckets covering the one hour observation window.
const BUCKET_COUNT: usize = 60;
const BUCKET_MS: u64 = 60_000;

/// Duty cycle limit in permille (1 = 0.1%) for the ERC 70-03 sub-band
/// containing `freq_hz`, or `None` where EU duty cycle rules do not apply
/// (e.g. the 915 MHz FCC band).
///
/// Within 863-870 MHz, frequencies outside a recognised sub-band get the most
/// restrictive limit (0.1%) rather than no limit.
pub fn eu_duty_cycle_permille(freq_hz: u32) -> Option<u16> {
    match freq_hz {
        433_050_000..=434_790_000 => Some(100), // 10%
        868_000_000..=868_600_000 => Some(10),  // 1%
        868_700_000..=869_200_000 => Some(1),   // 0.1%
        869_400_000..=869_650_000 => Some(100), // 10%
        869_700_000..=870_000_000 => Some(10),  // 1%
        f if (863_000_000..870_000_000).contains(&f) => Some(1), // 0.1% band-wide default
        _ => None,
    }
}

/// A transmission was refused because it would exceed the duty cycle budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DutyCycleExceeded {
    /// When enough budget will have freed up, or `None` if the packet's
    /// airtime exceeds the entire hourly allowance and can never be sent.
    pub retry_after_ms: Option<u64>,
}

/// Sliding-window airtime budget for one sub-band.
pub struct DutyCycleLimiter {
    /// Airtime spent per minute, in ms. `head` is the current minute.
    buckets: [u32; BUCKET_COUNT],
    head: usize,
    /// Start timestamp of the `head` bucket.
    head_start_ms: u64,
    /// Allowed airtime per sliding hour, or `None` for no limit.
    allowance_ms: Option<u32>,
}

impl DutyCycleLimiter {
    /// Limiter for the sub-band containing `freq_hz`.
    pub fn for_frequency(freq_hz: u32) -> Self {
        let allowance_ms = eu_duty_cycle_permille(freq_hz).map(|permille| permille as u32 * 3_600);
        Self {
            buckets: [0; BUCKET_COUNT],
            head: 0,
            head_start_ms: 0,
            allowance_ms,
        }
    }

    /// Airtime spent inside the current window, in ms.
    pub fn spent_ms(&self) -> u32 {
        self.buckets.iter().sum()
    }

    /// Rotate buckets so `head` covers `now_ms`, dropping expired minutes.
    fn advance(&mut self, now_ms: u64) {
        let elapsed_buckets = now_ms.saturating_sub(self.head_start_ms) / BUCKET_MS;
        if elapsed_buckets >= BUCKET_COUNT as u64 {
            self.buckets = [0; BUCKET_COUNT];
            self.head = 0;
            self.head_start_ms = now_ms;
            return;
        }
        for _ in 0..elapsed_buckets {
            self.head = (self.head + 1) % BUCKET_COUNT;
            self.buckets[self.head] = 0;
            self.head_start_ms += BUCKET_MS;
        }
    }

    /// Claim `airtime_ms` of budget for a transmission starting at `now_ms`
    /// (milliseconds from any monotonic origin, e.g. boot).
    ///
    /// On success the airtime is charged against the window. On refusal
    /// nothing is charged and the error says when to retry.
    pub fn try_claim(&mut self, now_ms: u64, airtime_ms: u32) -> Result<(), DutyCycleExceeded> {
        let Some(allowance_ms) = self.allowance_ms else {
            return Ok(());
        };
        if airtime_ms > allowance_ms {
            return Err(DutyCycleExceeded { retry_after_ms: None });
        }

        self.advance(now_ms);

        let spent = self.spent_ms();
        if spent + airtime_ms <= allowance_ms {
            self.buckets[self.head] += airtime_ms;
            return Ok(());
        }

        // Walk the buckets oldest-first to find when enough budget frees up.
        let mut freed = 0u32;
        for expired in 1..=BUCKET_COUNT {
            freed += self.buckets[(self.head + expired) % BUCKET_COUNT];
            if spent - freed + airtime_ms <= allowance_ms {
                let ready_at = self.head_start_ms + expired as u64 * BUCKET_MS;
                return Err(DutyCycleExceeded {
                    retry_after_ms: Some(ready_at.saturating_sub(now_ms)),
                });
            }
        }
        // Unreachable: expiring all buckets frees `spent`, and
        // `airtime_ms <= allowance_ms` was checked above.
        Err(DutyCycleExceeded { retry_after_ms: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10% band: 360 s allowance per hour.
    fn ten_percent_limiter() -> DutyCycleLimiter {
        DutyCycleLimiter::for_frequency(869_525_000)
    }

    #[test]
    fn sub_band_limits_match_erc_70_03() {
        assert_eq!(eu_duty_cycle_permille(869_525_000), Some(100));
        assert_eq!(eu_duty_cycle_permille(868_100_000), Some(10));
        assert_eq!(eu_duty_cycle_permille(868_900_000), Some(1));
        assert_eq!(eu_duty_cycle_permille(869_850_000), Some(10));
        assert_eq!(eu_duty_cycle_permille(864_000_000), Some(1));
        assert_eq!(eu_duty_cycle_permille(433_500_000), Some(100));
        assert_eq!(eu_duty_cycle_permille(915_000_000), None);
    }

    #[test]
    fn default_frequency_allows_360_seconds_per_hour() {
        let mut limiter = ten_percent_limiter();
        assert!(limiter.try_claim(0, 360_000).is_ok());
        assert_eq!(limiter.spent_ms(), 360_000);
    }

    #[test]
    fn claim_beyond_allowance_is_refused_and_not_charged() {
        let mut limiter = ten_percent_limiter();
        limiter.try_claim(0, 350_000).unwrap();

        let err = limiter.try_claim(1000, 20_000).unwrap_err();
        assert!(err.retry_after_ms.is_some());
        assert_eq!(limiter.spent_ms(), 350_000, "refused claim must not charge");
    }

    #[test]
    fn budget_frees_after_the_window_passes() {
        let mut limiter = ten_percent_limiter();
        limiter.try_claim(0, 360_000).unwrap();
        assert!(limiter.try_claim(1000, 1).is_err());

        // One sliding hour later (plus the bucket granularity) all is free.
        assert!(limiter.try_claim(61 * 60_000, 360_000).is_ok());
    }

    #[test]
    fn retry_hint_points_at_budget_release() {
        let mut limiter = ten_percent_limiter();
        limiter.try_claim(0, 360_000).unwrap();

        let err = limiter.try_claim(30 * 60_000, 7_000).unwrap_err();
        // The airtime sits in the minute-0 bucket, which leaves the window 60
        // minutes after it started; we ask 30 minutes in.
        assert_eq!(err.retry_after_ms, Some(30 * 60_000));
    }

    #[test]
    fn spending_spread_over_time_expires_incrementally() {
        let mut limiter = ten_percent_limiter();
        // 180 s in minute 0, 180 s in minute 30: the budget is gone...
        limiter.try_claim(0, 180_000).unwrap();
        limiter.try_claim(30 * 60_000, 180_000).unwrap();
        assert!(limiter.try_claim(31 * 60_000, 5_000).is_err());

        // ...at minute 61 the first spend has expired, freeing 180 s...
        assert!(limiter.try_claim(61 * 60_000, 5_000).is_ok());

        // ...but the minute-30 spend still binds a further large claim.
        assert!(limiter.try_claim(62 * 60_000, 180_000).is_err());
        assert!(limiter.try_claim(91 * 60_000, 175_000).is_ok());
    }

    #[test]
    fn packet_larger_than_hourly_allowance_is_never_sendable() {
        // 0.1% band: 3.6 s per hour, below a max-size SF12 packet (~7 s).
        let mut limiter = DutyCycleLimiter::for_frequency(868_900_000);
        let err = limiter.try_claim(0, 7_000).unwrap_err();
        assert_eq!(err.retry_after_ms, None);
    }

    #[test]
    fn non_eu_band_is_unlimited() {
        let mut limiter = DutyCycleLimiter::for_frequency(915_000_000);
        for i in 0..100 {
            assert!(limiter.try_claim(i * 1000, 360_000).is_ok());
        }
    }

    #[test]
    fn long_idle_resets_the_window() {
        let mut limiter = ten_percent_limiter();
        limiter.try_claim(0, 360_000).unwrap();
        // Days later the wrap-around maths must still hold.
        assert!(limiter.try_claim(72 * 3_600_000, 360_000).is_ok());
    }
}
