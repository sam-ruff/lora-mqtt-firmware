//! Embassy tasks module
//!
//! Contains all async tasks for the firmware, organised by functionality.

pub mod admin;
pub mod bridge;
pub mod gateway;
pub mod gateway_udp;
pub mod hub_ctrl;
pub mod led;
pub mod lora;
pub mod mqtt;
pub mod portal;
pub mod serial;
pub mod wifi;

pub use admin::{admin_task, AdminReceiver, ADMIN_CHANNEL};
pub use bridge::bridge_task;
pub use gateway::gateway_task;
pub use gateway_udp::gateway_udp_task;
pub use hub_ctrl::hub_ctrl_task;
pub use led::{led_task, LedReceiver, LedSender, LED_CHANNEL};
pub use lora::lora_task;
pub use mqtt::mqtt_task;
pub use portal::{portal_dhcp_task, portal_dns_task, portal_http_task};
pub use serial::{serial_reader_task, serial_writer_task, CommandReceiver, CommandSender};
pub use wifi::{net_watch_task, wifi_task};
