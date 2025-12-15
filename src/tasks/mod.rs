//! Embassy tasks module
//!
//! Contains all async tasks for the firmware, organised by functionality.

pub mod admin;
pub mod ble;
pub mod led;
pub mod lora;
pub mod serial;

pub use admin::{admin_task, AdminReceiver, ADMIN_CHANNEL};
pub use ble::ble_task;
pub use led::{led_task, LedReceiver, LedSender, LED_CHANNEL};
pub use lora::lora_task;
pub use serial::{serial_reader_task, serial_writer_task, CommandReceiver, CommandSender};
