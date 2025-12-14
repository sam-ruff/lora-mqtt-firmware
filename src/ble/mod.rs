//! Bluetooth Low Energy module
//!
//! Provides BLE connectivity using Nordic UART Service (NUS) for
//! command/response communication alongside serial.

pub mod service;

pub use service::NordicUartService;
