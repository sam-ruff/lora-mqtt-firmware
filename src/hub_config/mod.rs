//! Hub configuration: compile-time defaults with a flash override.
//!
//! Defaults are baked at build time from environment variables so a hub can
//! ship pre-provisioned; anything set at runtime over the host link is
//! persisted to the nvs flash partition and takes precedence on the next
//! boot. Settings apply at boot only (reboot after changing them).

#[cfg(feature = "embedded")]
pub mod store;

use hub_protocol::{
    AckStatus, GatewaySettings, HubCommand, HubMode, HubResponse, MqttSettings, WifiSettings,
};

/// Bump when the stored layout changes; a mismatch falls back to defaults.
pub const CONFIG_SCHEMA_VERSION: u8 = 1;

/// The full persisted hub configuration.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HubConfig {
    pub version: u8,
    pub mode: HubMode,
    pub wifi: WifiSettings,
    pub mqtt: MqttSettings,
    pub gateway: GatewaySettings,
}

#[cfg(feature = "embedded")]
impl<'a> sequential_storage::map::PostcardValue<'a> for HubConfig {}

impl Default for HubConfig {
    /// The compile-time defaults, from `HUB_*` environment variables at build
    /// time (all optional): `HUB_WIFI_SSID`, `HUB_WIFI_PASSWORD`,
    /// `HUB_MQTT_HOST`, `HUB_MQTT_PORT`, `HUB_MQTT_CLIENT_ID`, `HUB_MODE`
    /// (`bridge` / `gateway`), `HUB_GW_HOST`, `HUB_GW_PORT`.
    fn default() -> Self {
        let mut gateway = GatewaySettings {
            host: baked_str(option_env!("HUB_GW_HOST")),
            ..GatewaySettings::default()
        };
        if let Some(port) = parse_port(option_env!("HUB_GW_PORT")) {
            gateway.port = port;
        }
        Self {
            version: CONFIG_SCHEMA_VERSION,
            mode: parse_mode(option_env!("HUB_MODE")),
            wifi: WifiSettings {
                ssid: baked_str(option_env!("HUB_WIFI_SSID")),
                password: baked_str(option_env!("HUB_WIFI_PASSWORD")),
            },
            mqtt: MqttSettings {
                host: baked_str(option_env!("HUB_MQTT_HOST")),
                port: parse_port(option_env!("HUB_MQTT_PORT")).unwrap_or(1883),
                client_id: baked_str(option_env!("HUB_MQTT_CLIENT_ID")),
            },
            gateway,
        }
    }
}

impl HubConfig {
    /// WiFi is only attempted once an SSID exists.
    pub fn wifi_provisioned(&self) -> bool {
        !self.wifi.ssid.is_empty()
    }
}

/// Pick the active configuration: a stored value wins over the baked
/// defaults, unless its schema version does not match.
pub fn select_config(stored: Option<HubConfig>) -> HubConfig {
    match stored {
        Some(config) if config.version == CONFIG_SCHEMA_VERSION => config,
        _ => HubConfig::default(),
    }
}

/// Storage operation a hub command requires after validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigAction {
    /// Read-only command; nothing to persist.
    None,
    /// Persist the updated config.
    Save,
    /// Erase the stored config.
    Clear,
}

/// Apply a hub command to the pending configuration. On a validation error
/// the config is left untouched and the ack status to report is returned.
pub fn apply_command(
    pending: &mut HubConfig,
    command: &HubCommand,
) -> Result<ConfigAction, AckStatus> {
    match command {
        HubCommand::SetWifiConfig(wifi) => {
            pending.wifi = wifi.clone();
            Ok(ConfigAction::Save)
        }
        HubCommand::SetMqttConfig(mqtt) => {
            if mqtt.port == 0 {
                return Err(AckStatus::InvalidValue);
            }
            pending.mqtt = mqtt.clone();
            Ok(ConfigAction::Save)
        }
        HubCommand::SetGatewayConfig(gw) => {
            if !gateway_settings_valid(gw) {
                return Err(AckStatus::InvalidValue);
            }
            pending.gateway = gw.clone();
            Ok(ConfigAction::Save)
        }
        HubCommand::SetMode { mode } => {
            pending.mode = *mode;
            Ok(ConfigAction::Save)
        }
        HubCommand::ClearNetworkConfig => {
            *pending = HubConfig::default();
            Ok(ConfigAction::Clear)
        }
        HubCommand::GetNetworkConfig | HubCommand::GetHubStatus => Ok(ConfigAction::None),
    }
}

fn gateway_settings_valid(gw: &GatewaySettings) -> bool {
    let freq_ok = (150_000_000..=960_000_000).contains(&gw.frequency_hz);
    let sf_ok = (7..=12).contains(&gw.spreading_factor);
    let bw_ok = matches!(gw.bandwidth_khz, 125 | 250 | 500);
    let cr_ok = (5..=8).contains(&gw.coding_rate);
    freq_ok && sf_ok && bw_ok && cr_ok && gw.port != 0
}

/// The `GetNetworkConfig` reply for a configuration. Never includes the WiFi
/// password.
pub fn network_config_response(config: &HubConfig) -> HubResponse {
    HubResponse::NetworkConfig {
        mode: config.mode,
        wifi_ssid: config.wifi.ssid.clone(),
        mqtt: config.mqtt.clone(),
        gateway: config.gateway.clone(),
    }
}

/// A baked string, empty when unset or over the field cap.
fn baked_str<const N: usize>(value: Option<&str>) -> heapless::String<N> {
    value
        .and_then(|v| heapless::String::try_from(v).ok())
        .unwrap_or_default()
}

fn parse_port(value: Option<&str>) -> Option<u16> {
    value.and_then(|v| v.parse().ok())
}

fn parse_mode(value: Option<&str>) -> HubMode {
    match value {
        Some("gateway") => HubMode::LorawanGateway,
        _ => HubMode::Bridge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(version: u8) -> HubConfig {
        let mut config = HubConfig {
            version,
            ..HubConfig::default()
        };
        config.wifi.ssid = heapless::String::try_from("StoredNet").unwrap();
        config
    }

    #[test]
    fn postcard_round_trip() {
        let config = stored(CONFIG_SCHEMA_VERSION);
        let mut buf = [0u8; 512];
        let bytes = postcard::to_slice(&config, &mut buf).unwrap();
        let decoded: HubConfig = postcard::from_bytes(bytes).unwrap();
        assert_eq!(decoded, config);
    }

    #[test]
    fn stored_config_wins_over_defaults() {
        let active = select_config(Some(stored(CONFIG_SCHEMA_VERSION)));
        assert_eq!(active.wifi.ssid.as_str(), "StoredNet");
    }

    #[test]
    fn version_mismatch_falls_back_to_defaults() {
        let active = select_config(Some(stored(CONFIG_SCHEMA_VERSION + 1)));
        assert_ne!(active.wifi.ssid.as_str(), "StoredNet");
    }

    #[test]
    fn no_stored_config_uses_defaults() {
        let active = select_config(None);
        assert_eq!(active, HubConfig::default());
    }

    #[test]
    fn baked_string_over_cap_is_empty() {
        let over: heapless::String<4> = baked_str(Some("too long for four"));
        assert!(over.is_empty());
    }

    #[test]
    fn port_and_mode_parsing() {
        assert_eq!(parse_port(Some("1883")), Some(1883));
        assert_eq!(parse_port(Some("nope")), None);
        assert_eq!(parse_port(None), None);
        assert_eq!(parse_mode(Some("gateway")), HubMode::LorawanGateway);
        assert_eq!(parse_mode(Some("bridge")), HubMode::Bridge);
        assert_eq!(parse_mode(None), HubMode::Bridge);
    }

    #[test]
    fn set_commands_update_pending_and_request_save() {
        let mut pending = HubConfig::default();
        let wifi = WifiSettings {
            ssid: heapless::String::try_from("Net").unwrap(),
            password: heapless::String::try_from("pw").unwrap(),
        };
        let action = apply_command(&mut pending, &HubCommand::SetWifiConfig(wifi.clone()));
        assert_eq!(action, Ok(ConfigAction::Save));
        assert_eq!(pending.wifi, wifi);

        let action =
            apply_command(&mut pending, &HubCommand::SetMode { mode: HubMode::LorawanGateway });
        assert_eq!(action, Ok(ConfigAction::Save));
        assert_eq!(pending.mode, HubMode::LorawanGateway);
    }

    #[test]
    fn invalid_values_are_rejected_and_leave_config_untouched() {
        let mut pending = HubConfig::default();
        let before = pending.clone();

        let mqtt = MqttSettings { port: 0, ..MqttSettings::default() };
        assert_eq!(
            apply_command(&mut pending, &HubCommand::SetMqttConfig(mqtt)),
            Err(AckStatus::InvalidValue)
        );

        let gw = GatewaySettings { spreading_factor: 6, ..GatewaySettings::default() };
        assert_eq!(
            apply_command(&mut pending, &HubCommand::SetGatewayConfig(gw)),
            Err(AckStatus::InvalidValue)
        );

        let gw = GatewaySettings { bandwidth_khz: 100, ..GatewaySettings::default() };
        assert_eq!(
            apply_command(&mut pending, &HubCommand::SetGatewayConfig(gw)),
            Err(AckStatus::InvalidValue)
        );

        assert_eq!(pending, before);
    }

    #[test]
    fn clear_resets_to_defaults_and_requests_clear() {
        let mut pending = stored(CONFIG_SCHEMA_VERSION);
        let action = apply_command(&mut pending, &HubCommand::ClearNetworkConfig);
        assert_eq!(action, Ok(ConfigAction::Clear));
        assert_eq!(pending, HubConfig::default());
    }

    #[test]
    fn network_config_response_never_contains_password() {
        let mut config = HubConfig::default();
        config.wifi.password = heapless::String::try_from("secret").unwrap();
        let response = network_config_response(&config);
        let frame = hub_protocol::encode_response(&response);
        let haystack: &[u8] = &frame;
        assert!(!haystack.windows(6).any(|w| w == b"secret"));
    }
}
