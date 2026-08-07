//! Live hub statistics, shared between tasks as lock-free atomics.
//!
//! The WiFi/MQTT tasks write their side; the hub control task snapshots
//! everything for `GetHubStatus`.

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use hub_protocol::{HubStatus, LinkState};

/// Global statistics instance.
pub static HUB_STATS: HubStats = HubStats::new();

pub struct HubStats {
    wifi_state: AtomicU8,
    mqtt_state: AtomicU8,
    /// Station IPv4 address as big-endian octets packed into a u32.
    ip: AtomicU32,
    uplink: AtomicU32,
    downlink: AtomicU32,
    dropped: AtomicU32,
}

impl Default for HubStats {
    fn default() -> Self {
        Self::new()
    }
}

impl HubStats {
    pub const fn new() -> Self {
        Self {
            wifi_state: AtomicU8::new(LinkState::Unprovisioned as u8),
            mqtt_state: AtomicU8::new(LinkState::Unprovisioned as u8),
            ip: AtomicU32::new(0),
            uplink: AtomicU32::new(0),
            downlink: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
        }
    }

    pub fn set_wifi_state(&self, state: LinkState) {
        self.wifi_state.store(state as u8, Ordering::Relaxed);
    }

    pub fn set_mqtt_state(&self, state: LinkState) {
        self.mqtt_state.store(state as u8, Ordering::Relaxed);
    }

    pub fn set_ip(&self, octets: [u8; 4]) {
        self.ip.store(u32::from_be_bytes(octets), Ordering::Relaxed);
    }

    pub fn clear_ip(&self) {
        self.ip.store(0, Ordering::Relaxed);
    }

    pub fn count_uplink(&self) {
        self.uplink.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_downlink(&self) {
        self.downlink.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_dropped(&self, n: u32) {
        self.dropped.fetch_add(n, Ordering::Relaxed);
    }

    /// Snapshot for the wire; the caller supplies uptime so this stays free
    /// of any clock dependency.
    pub fn snapshot(&self, uptime_secs: u32) -> HubStatus {
        HubStatus {
            wifi_state: LinkState::from_byte(self.wifi_state.load(Ordering::Relaxed))
                .unwrap_or_default(),
            ip: self.ip.load(Ordering::Relaxed).to_be_bytes(),
            mqtt_state: LinkState::from_byte(self.mqtt_state.load(Ordering::Relaxed))
                .unwrap_or_default(),
            uplink_count: self.uplink.load(Ordering::Relaxed),
            downlink_count: self.downlink.load(Ordering::Relaxed),
            dropped_count: self.dropped.load(Ordering::Relaxed),
            uptime_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reflects_updates() {
        let stats = HubStats::new();
        stats.set_wifi_state(LinkState::Connected);
        stats.set_mqtt_state(LinkState::Connecting);
        stats.set_ip([192, 168, 1, 42]);
        stats.count_uplink();
        stats.count_uplink();
        stats.count_downlink();
        stats.count_dropped(3);

        let snap = stats.snapshot(120);
        assert_eq!(snap.wifi_state, LinkState::Connected);
        assert_eq!(snap.mqtt_state, LinkState::Connecting);
        assert_eq!(snap.ip, [192, 168, 1, 42]);
        assert_eq!(snap.uplink_count, 2);
        assert_eq!(snap.downlink_count, 1);
        assert_eq!(snap.dropped_count, 3);
        assert_eq!(snap.uptime_secs, 120);
    }
}
