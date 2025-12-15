//! BLE client for communicating with WalkieTextie device via Nordic UART Service.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::time::timeout;
use uuid::Uuid;

use crate::protocol::{build_command, cobs_decode, parse_response, CommandId, Response};

/// Nordic UART Service UUIDs
const NUS_SERVICE_UUID: Uuid = Uuid::from_u128(0x6e400001_b5a3_f393_e0a9_e50e24dcca9e);
const NUS_RX_UUID: Uuid = Uuid::from_u128(0x6e400002_b5a3_f393_e0a9_e50e24dcca9e); // Write to device
const NUS_TX_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e); // Notify from device

/// BLE client for communicating with the WalkieTextie device.
pub struct BleClient {
    peripheral: Peripheral,
    rx_char: Characteristic,
    tx_char: Characteristic,
    /// Buffer for accumulating notification data
    notification_buffer: Arc<Mutex<Vec<u8>>>,
}

impl BleClient {
    /// Scan for a device by name and connect.
    pub async fn connect_by_name(name: &str, scan_timeout: Duration) -> Result<Self> {
        let manager = Manager::new().await?;
        let adapters = manager.adapters().await?;
        let adapter = adapters
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No Bluetooth adapters found"))?;

        // Start scanning
        adapter.start_scan(ScanFilter::default()).await?;

        // Wait for the device to appear
        let peripheral = Self::find_device_by_name(&adapter, name, scan_timeout).await?;

        adapter.stop_scan().await?;

        // Connect to the device
        peripheral.connect().await?;

        // Discover services
        peripheral.discover_services().await?;

        // Find NUS characteristics
        let characteristics = peripheral.characteristics();

        let rx_char = characteristics
            .iter()
            .find(|c| c.uuid == NUS_RX_UUID)
            .cloned()
            .ok_or_else(|| anyhow!("NUS RX characteristic not found"))?;

        let tx_char = characteristics
            .iter()
            .find(|c| c.uuid == NUS_TX_UUID)
            .cloned()
            .ok_or_else(|| anyhow!("NUS TX characteristic not found"))?;

        // Subscribe to notifications on TX characteristic
        peripheral.subscribe(&tx_char).await?;

        let notification_buffer = Arc::new(Mutex::new(Vec::new()));

        // Spawn notification handler
        let buffer_clone = notification_buffer.clone();
        let peripheral_clone = peripheral.clone();
        tokio::spawn(async move {
            let mut stream = match peripheral_clone.notifications().await {
                Ok(s) => s,
                Err(_) => return,
            };

            while let Some(data) = stream.next().await {
                if data.uuid == NUS_TX_UUID {
                    let mut buf = buffer_clone.lock().await;
                    buf.extend_from_slice(&data.value);
                }
            }
        });

        Ok(Self {
            peripheral,
            rx_char,
            tx_char,
            notification_buffer,
        })
    }

    /// Find a device by name within the scan timeout.
    async fn find_device_by_name(
        adapter: &Adapter,
        name: &str,
        scan_timeout: Duration,
    ) -> Result<Peripheral> {
        let start = std::time::Instant::now();

        while start.elapsed() < scan_timeout {
            let peripherals = adapter.peripherals().await?;

            for peripheral in peripherals {
                if let Some(props) = peripheral.properties().await? {
                    if let Some(local_name) = props.local_name {
                        if local_name == name {
                            return Ok(peripheral);
                        }
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Err(anyhow!("Device '{}' not found within timeout", name))
    }

    /// Send a command and wait for response.
    pub async fn send_command(
        &self,
        cmd_id: CommandId,
        payload: &[u8],
        response_timeout: Duration,
    ) -> Result<Response> {
        // Clear any pending notifications
        {
            let mut buf = self.notification_buffer.lock().await;
            buf.clear();
        }

        // Build and send command
        let frame = build_command(cmd_id, payload);
        self.peripheral
            .write(&self.rx_char, &frame, WriteType::WithoutResponse)
            .await?;

        // Wait for response
        self.wait_for_response(response_timeout).await
    }

    /// Wait for a notification response with timeout.
    pub async fn wait_for_response(&self, response_timeout: Duration) -> Result<Response> {
        let result = timeout(response_timeout, async {
            loop {
                // Check for complete frame in buffer
                let mut buf = self.notification_buffer.lock().await;

                // Look for zero delimiter
                if let Some(pos) = buf.iter().position(|&b| b == 0x00) {
                    if pos > 0 {
                        // Extract frame (without delimiter)
                        let frame_data: Vec<u8> = buf.drain(..=pos).collect();
                        drop(buf); // Release lock before processing

                        // Decode COBS (with delimiter)
                        let decoded = cobs_decode(&frame_data)?;
                        return parse_response(&decoded);
                    } else {
                        // Empty frame, skip the delimiter
                        buf.remove(0);
                    }
                } else {
                    drop(buf);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        })
        .await;

        match result {
            Ok(response) => response,
            Err(_) => Err(anyhow!("Timeout waiting for BLE response")),
        }
    }

    /// Try to read an unsolicited response (non-blocking check).
    pub async fn try_read_response(&self, timeout_duration: Duration) -> Result<Option<Response>> {
        match timeout(timeout_duration, self.wait_for_response(timeout_duration)).await {
            Ok(Ok(response)) => Ok(Some(response)),
            Ok(Err(e)) => {
                if e.to_string().contains("Timeout") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
            Err(_) => Ok(None), // Timeout
        }
    }

    /// Wait for an unsolicited RxPacket response.
    pub async fn wait_for_rx_packet(&self, timeout_duration: Duration) -> Result<Response> {
        self.wait_for_response(timeout_duration).await
    }

    /// Send LoRa TX command with data.
    pub async fn lora_tx(&self, data: &[u8], response_timeout: Duration) -> Result<Response> {
        self.send_command(CommandId::LoraTx, data, response_timeout)
            .await
    }

    /// Disconnect from the device.
    pub async fn disconnect(&self) -> Result<()> {
        self.peripheral.unsubscribe(&self.tx_char).await?;
        self.peripheral.disconnect().await?;
        Ok(())
    }

    /// Clear any pending notifications from the buffer.
    pub async fn clear_buffer(&self) {
        let mut buf = self.notification_buffer.lock().await;
        buf.clear();
    }
}

impl Drop for BleClient {
    fn drop(&mut self) {
        // Note: We can't do async cleanup in Drop, but the peripheral
        // will be disconnected when it goes out of scope anyway
    }
}
