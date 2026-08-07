//! Hub command and response types.

use heapless::String;

use crate::{MAX_CLIENT_ID_LEN, MAX_HOST_LEN, MAX_PASSWORD_LEN, MAX_SSID_LEN};

/// Hub command IDs (host -> hub), reserved block 0x20..=0x2F.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubCommandId {
    /// Set WiFi credentials.
    SetWifiConfig = 0x20,
    /// Set MQTT broker settings.
    SetMqttConfig = 0x21,
    /// Read the stored network configuration (password never included).
    GetNetworkConfig = 0x22,
    /// Read live hub status (link states, counters, uptime).
    GetHubStatus = 0x23,
    /// Select the operating mode (bridge / LoRaWAN gateway).
    SetMode = 0x24,
    /// Erase all stored network configuration.
    ClearNetworkConfig = 0x25,
    /// Set LoRaWAN gateway radio and network-server settings.
    SetGatewayConfig = 0x26,
}

impl HubCommandId {
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x20 => Some(Self::SetWifiConfig),
            0x21 => Some(Self::SetMqttConfig),
            0x22 => Some(Self::GetNetworkConfig),
            0x23 => Some(Self::GetHubStatus),
            0x24 => Some(Self::SetMode),
            0x25 => Some(Self::ClearNetworkConfig),
            0x26 => Some(Self::SetGatewayConfig),
            _ => None,
        }
    }
}

/// Hub response IDs (hub -> host), reserved block 0x20..=0x2F.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubResponseId {
    NetworkConfig = 0x20,
    HubStatus = 0x21,
    ConfigAck = 0x22,
}

impl HubResponseId {
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x20 => Some(Self::NetworkConfig),
            0x21 => Some(Self::HubStatus),
            0x22 => Some(Self::ConfigAck),
            _ => None,
        }
    }
}

/// Hub operating mode.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HubMode {
    /// Raw LoRa <-> MQTT bridge (walkie-textie packets).
    #[default]
    Bridge = 0,
    /// Single-channel LoRaWAN gateway (Semtech UDP packet forwarder).
    LorawanGateway = 1,
}

impl HubMode {
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Bridge),
            1 => Some(Self::LorawanGateway),
            _ => None,
        }
    }
}

/// State of a network link (WiFi association or MQTT session).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkState {
    /// No configuration stored; the link is not attempted.
    #[default]
    Unprovisioned = 0,
    Connecting = 1,
    Connected = 2,
    Disconnected = 3,
}

impl LinkState {
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Unprovisioned),
            1 => Some(Self::Connecting),
            2 => Some(Self::Connected),
            3 => Some(Self::Disconnected),
            _ => None,
        }
    }
}

/// WiFi credentials.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WifiSettings {
    pub ssid: String<MAX_SSID_LEN>,
    pub password: String<MAX_PASSWORD_LEN>,
}

/// MQTT broker settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MqttSettings {
    pub host: String<MAX_HOST_LEN>,
    pub port: u16,
    /// Empty means derive `wt-hub-XXXXXX` from the device id.
    pub client_id: String<MAX_CLIENT_ID_LEN>,
}

/// LoRaWAN gateway radio and network-server settings.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GatewaySettings {
    /// Uplink listen frequency.
    pub frequency_hz: u32,
    pub spreading_factor: u8,
    pub bandwidth_khz: u32,
    pub coding_rate: u8,
    /// Semtech UDP network server (e.g. ChirpStack gateway bridge).
    pub host: String<MAX_HOST_LEN>,
    pub port: u16,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        // EU868 single-channel gateway convention: 868.1 MHz SF7BW125 CR4/5.
        Self {
            frequency_hz: 868_100_000,
            spreading_factor: 7,
            bandwidth_khz: 125,
            coding_rate: 5,
            host: String::new(),
            port: 1700,
        }
    }
}

/// Live hub status snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HubStatus {
    pub wifi_state: LinkState,
    /// Station IPv4 address, zeroes when not connected.
    pub ip: [u8; 4],
    pub mqtt_state: LinkState,
    /// LoRa packets published to MQTT / forwarded to the network server.
    pub uplink_count: u32,
    /// LoRa transmissions performed for MQTT / network-server requests.
    pub downlink_count: u32,
    /// Messages dropped because a queue was full or a link was down.
    pub dropped_count: u32,
    pub uptime_secs: u32,
}

/// A hub command parsed from (or to be sent over) the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubCommand {
    SetWifiConfig(WifiSettings),
    SetMqttConfig(MqttSettings),
    GetNetworkConfig,
    GetHubStatus,
    SetMode { mode: HubMode },
    ClearNetworkConfig,
    SetGatewayConfig(GatewaySettings),
}

impl HubCommand {
    pub fn id(&self) -> HubCommandId {
        match self {
            HubCommand::SetWifiConfig(_) => HubCommandId::SetWifiConfig,
            HubCommand::SetMqttConfig(_) => HubCommandId::SetMqttConfig,
            HubCommand::GetNetworkConfig => HubCommandId::GetNetworkConfig,
            HubCommand::GetHubStatus => HubCommandId::GetHubStatus,
            HubCommand::SetMode { .. } => HubCommandId::SetMode,
            HubCommand::ClearNetworkConfig => HubCommandId::ClearNetworkConfig,
            HubCommand::SetGatewayConfig(_) => HubCommandId::SetGatewayConfig,
        }
    }
}

/// Outcome of a set/clear configuration command.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckStatus {
    Ok = 0,
    /// The value was rejected (e.g. port 0, out-of-range radio parameter).
    InvalidValue = 1,
    /// Persisting to flash failed; the setting is not stored.
    StorageError = 2,
}

impl AckStatus {
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Ok),
            1 => Some(Self::InvalidValue),
            2 => Some(Self::StorageError),
            _ => None,
        }
    }
}

/// A hub response parsed from (or to be sent over) the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubResponse {
    /// The stored configuration. The WiFi password is never echoed.
    NetworkConfig {
        mode: HubMode,
        wifi_ssid: String<MAX_SSID_LEN>,
        mqtt: MqttSettings,
        gateway: GatewaySettings,
    },
    HubStatus(HubStatus),
    /// Acknowledges a set/clear command.
    ConfigAck { status: AckStatus },
}
