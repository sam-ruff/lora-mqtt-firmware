//! Bluetooth Low Energy module
//!
//! Provides BLE connectivity using Nordic UART Service (NUS) for
//! command/response communication alongside serial.

pub mod service;
pub mod tasks;

pub use service::NordicUartService;
pub use tasks::ble_task;
