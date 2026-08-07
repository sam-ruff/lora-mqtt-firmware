//! Hub provisioning API client.
//!
//! All network access sits behind [`HubApi`] so the orchestration in
//! [`crate::engine`] unit-tests with a mocked hub, and the app's dev mode
//! runs against [`FakeHub`] with no hardware.

use std::sync::Mutex;
use std::time::Duration;

use crate::models::{ApplyOutcome, ConfigUpdate, HubConfigView, HubStatusView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// The hub did not answer (wrong network, wrong address, asleep).
    Unreachable(String),
    /// The hub answered but rejected the request.
    Rejected(String),
}

impl core::fmt::Display for ApiError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ApiError::Unreachable(detail) => write!(f, "hub unreachable: {detail}"),
            ApiError::Rejected(detail) => write!(f, "hub rejected the request: {detail}"),
        }
    }
}

#[cfg_attr(test, mockall::automock)]
pub trait HubApi {
    fn fetch_config(&self) -> Result<HubConfigView, ApiError>;
    fn fetch_status(&self) -> Result<HubStatusView, ApiError>;
    fn apply_config(&self, update: ConfigUpdate) -> Result<ApplyOutcome, ApiError>;
}

/// The real client over plain HTTP.
pub struct HttpHubApi {
    base_url: String,
    agent: ureq::Agent,
}

impl HttpHubApi {
    pub fn new(base_url: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(6)))
            .build();
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            agent: config.new_agent(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

impl HubApi for HttpHubApi {
    fn fetch_config(&self) -> Result<HubConfigView, ApiError> {
        self.agent
            .get(&self.url("/api/config"))
            .call()
            .map_err(|e| ApiError::Unreachable(e.to_string()))?
            .body_mut()
            .read_json()
            .map_err(|e| ApiError::Rejected(e.to_string()))
    }

    fn fetch_status(&self) -> Result<HubStatusView, ApiError> {
        self.agent
            .get(&self.url("/api/status"))
            .call()
            .map_err(|e| ApiError::Unreachable(e.to_string()))?
            .body_mut()
            .read_json()
            .map_err(|e| ApiError::Rejected(e.to_string()))
    }

    fn apply_config(&self, update: ConfigUpdate) -> Result<ApplyOutcome, ApiError> {
        let mut response = self
            .agent
            .post(&self.url("/api/config"))
            .send_json(&update)
            .map_err(|e| match e {
                // 4xx/5xx surface as status errors: the hub spoke, but said no.
                ureq::Error::StatusCode(code) => ApiError::Rejected(format!("HTTP {code}")),
                other => ApiError::Unreachable(other.to_string()),
            })?;
        response
            .body_mut()
            .read_json()
            .map_err(|e| ApiError::Rejected(e.to_string()))
    }
}

/// In-memory hub for the app's dev/mock mode: behaves like a freshly booted,
/// unprovisioned hub and remembers what is saved to it.
pub struct FakeHub {
    state: Mutex<HubConfigView>,
}

impl FakeHub {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(HubConfigView {
                mode: "bridge".into(),
                wifi_ssid: String::new(),
                mqtt_host: String::new(),
                mqtt_port: 1883,
                client_id: String::new(),
                gw_freq_hz: 868_100_000,
                gw_sf: 7,
                gw_bw_khz: 125,
                gw_cr: 5,
                gw_host: String::new(),
                gw_port: 1700,
            }),
        }
    }
}

impl Default for FakeHub {
    fn default() -> Self {
        Self::new()
    }
}

impl HubApi for FakeHub {
    fn fetch_config(&self) -> Result<HubConfigView, ApiError> {
        Ok(self.state.lock().unwrap_or_else(|p| p.into_inner()).clone())
    }

    fn fetch_status(&self) -> Result<HubStatusView, ApiError> {
        let provisioned = !self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .wifi_ssid
            .is_empty();
        Ok(HubStatusView {
            wifi_state: if provisioned { 2 } else { 0 },
            mqtt_state: if provisioned { 2 } else { 0 },
            ip: if provisioned { "192.168.1.50".into() } else { "0.0.0.0".into() },
            uplink: 12,
            downlink: 3,
            dropped: 0,
            uptime_secs: 4200,
        })
    }

    fn apply_config(&self, update: ConfigUpdate) -> Result<ApplyOutcome, ApiError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(ssid) = update.wifi_ssid {
            state.wifi_ssid = ssid;
        }
        if let Some(host) = update.mqtt_host {
            state.mqtt_host = host;
        }
        if let Some(port) = update.mqtt_port {
            state.mqtt_port = port;
        }
        if let Some(id) = update.client_id {
            state.client_id = id;
        }
        if let Some(mode) = update.mode {
            state.mode = mode;
        }
        if let Some(freq) = update.gw_freq_hz {
            state.gw_freq_hz = freq;
        }
        if let Some(sf) = update.gw_sf {
            state.gw_sf = sf;
        }
        if let Some(bw) = update.gw_bw_khz {
            state.gw_bw_khz = bw;
        }
        if let Some(cr) = update.gw_cr {
            state.gw_cr = cr;
        }
        if let Some(host) = update.gw_host {
            state.gw_host = host;
        }
        if let Some(port) = update.gw_port {
            state.gw_port = port;
        }
        Ok(ApplyOutcome { ok: true, rebooting: true })
    }
}
