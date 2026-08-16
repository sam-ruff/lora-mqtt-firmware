//! Decision logic for the WiFi station supervisor, kept clock- and
//! driver-free so the retry, backoff and give-up rules are host-testable.
//! The wifi task turns each [`StationAction`] into driver calls.

use crate::net::backoff::Backoff;

/// Consecutive station failures before falling back to the provisioning AP
/// (with exponential backoff this spans several minutes).
pub const MAX_STA_FAILURES: u32 = 10;

/// What the wifi task should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StationAction {
    /// Apply the station config (which also starts the driver).
    Start,
    /// Associate with the configured network.
    Connect,
    /// Too many failures: hand the radio to the provisioning access point.
    FallbackToAccessPoint,
}

/// Tracks whether the station driver is up and how many attempts have failed.
pub struct StationSupervisor {
    backoff: Backoff,
    failures: u32,
    started: bool,
}

impl StationSupervisor {
    pub const fn new() -> Self {
        Self {
            backoff: Backoff::new(),
            failures: 0,
            started: false,
        }
    }

    /// The next step for the current state.
    pub fn next_action(&self) -> StationAction {
        if self.failures >= MAX_STA_FAILURES {
            return StationAction::FallbackToAccessPoint;
        }
        if !self.started {
            return StationAction::Start;
        }
        StationAction::Connect
    }

    /// The station config was applied and the driver is running.
    pub fn started(&mut self) {
        self.started = true;
    }

    /// A start or connect attempt failed; returns the seconds to wait before
    /// the next attempt.
    pub fn failed(&mut self) -> u32 {
        self.failures += 1;
        self.backoff.next_secs()
    }

    /// Associated with the access point: the failure budget starts afresh.
    pub fn connected(&mut self) {
        self.failures = 0;
        self.backoff.reset();
    }
}

impl Default for StationSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_before_connecting() {
        let mut sup = StationSupervisor::new();
        assert_eq!(sup.next_action(), StationAction::Start);
        sup.started();
        assert_eq!(sup.next_action(), StationAction::Connect);
    }

    #[test]
    fn start_failure_is_retried_with_backoff() {
        let mut sup = StationSupervisor::new();
        assert_eq!(sup.next_action(), StationAction::Start);
        assert_eq!(sup.failed(), 1);
        assert_eq!(sup.next_action(), StationAction::Start);
        assert_eq!(sup.failed(), 2);
        assert_eq!(sup.next_action(), StationAction::Start);
        sup.started();
        assert_eq!(sup.next_action(), StationAction::Connect);
    }

    #[test]
    fn connect_failures_back_off_and_reset_on_success() {
        let mut sup = StationSupervisor::new();
        sup.started();
        assert_eq!(sup.failed(), 1);
        assert_eq!(sup.failed(), 2);
        assert_eq!(sup.failed(), 4);
        sup.connected();
        assert_eq!(sup.next_action(), StationAction::Connect);
        // Backoff restarts and the failure budget is full again.
        assert_eq!(sup.failed(), 1);
        for _ in 0..MAX_STA_FAILURES - 2 {
            sup.failed();
        }
        assert_eq!(sup.next_action(), StationAction::Connect);
    }

    #[test]
    fn falls_back_to_access_point_after_max_failures() {
        let mut sup = StationSupervisor::new();
        sup.started();
        for _ in 0..MAX_STA_FAILURES - 1 {
            sup.failed();
            assert_eq!(sup.next_action(), StationAction::Connect);
        }
        sup.failed();
        assert_eq!(sup.next_action(), StationAction::FallbackToAccessPoint);
    }

    #[test]
    fn start_and_connect_failures_share_the_budget() {
        let mut sup = StationSupervisor::new();
        for _ in 0..MAX_STA_FAILURES / 2 {
            sup.failed();
        }
        sup.started();
        for _ in 0..MAX_STA_FAILURES / 2 {
            sup.failed();
        }
        assert_eq!(sup.next_action(), StationAction::FallbackToAccessPoint);
    }

    #[test]
    fn a_reconnect_after_drop_keeps_the_driver_started() {
        let mut sup = StationSupervisor::new();
        sup.started();
        sup.connected();
        // A later disconnect only needs a fresh connect, never a restart.
        assert_eq!(sup.next_action(), StationAction::Connect);
    }
}
