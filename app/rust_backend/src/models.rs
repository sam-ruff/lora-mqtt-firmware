//! Typed views of the hub's provisioning API JSON.

use serde::{Deserialize, Serialize};

/// `GET /api/config` (the hub never includes the WiFi password).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubConfigView {
    pub mode: String,
    pub wifi_ssid: String,
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub client_id: String,
    pub gw_freq_hz: u32,
    pub gw_sf: u8,
    pub gw_bw_khz: u32,
    pub gw_cr: u8,
    pub gw_host: String,
    pub gw_port: u16,
}

/// `GET /api/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubStatusView {
    pub wifi_state: u8,
    pub mqtt_state: u8,
    pub ip: String,
    pub uplink: u32,
    pub downlink: u32,
    pub dropped: u32,
    pub uptime_secs: u32,
}

/// `POST /api/config` body: only the present fields change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wifi_ssid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wifi_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mqtt_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mqtt_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gw_freq_hz: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gw_sf: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gw_bw_khz: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gw_cr: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gw_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gw_port: Option<u16>,
}

/// `POST /api/config` reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyOutcome {
    pub ok: bool,
    #[serde(default)]
    pub rebooting: bool,
}

/// Both halves of what the connect screen needs in one round trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubSnapshot {
    pub config: HubConfigView,
    pub status: HubStatusView,
}
