//! SoftAP provisioning portal.
//!
//! When the hub has no WiFi credentials (or repeatedly fails to join the
//! configured network), the WiFi task starts an open access point named
//! `WalkieTextieHub-XXXXXX` instead of the station. A phone that joins gets
//! an address from the one-lease DHCP server, every DNS name resolves to the
//! portal, and the embedded page (or an app speaking the JSON API) sets WiFi,
//! MQTT, mode and gateway settings. Saving persists to flash and restarts
//! the hub into the configured network.

pub mod dhcp;
pub mod dns;
pub mod http;

/// Portal / gateway address on the SoftAP network.
pub const AP_IP: [u8; 4] = [192, 168, 4, 1];
/// The single DHCP lease handed to the joining phone.
pub const CLIENT_IP: [u8; 4] = [192, 168, 4, 2];
pub const NETMASK: [u8; 4] = [255, 255, 255, 0];
