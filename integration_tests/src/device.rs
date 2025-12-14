//! Device communication client.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use serialport::SerialPort;

use crate::protocol::{build_command, cobs_decode, cobs_encode, build_command_payload, parse_response, CommandId, Response, ResponseId};

/// Find available data ports by scanning ttyACM devices and testing with GetVersion.
/// Returns a list of port names that respond to GetVersion (these are the data ports).
pub fn find_data_ports() -> Result<Vec<String>> {
    let ports = serialport::available_ports()?;
    let mut data_ports = Vec::new();

    for port_info in ports {
        // Filter to ttyACM devices (CDC-ACM)
        if !port_info.port_name.contains("ttyACM") {
            continue;
        }

        // Try to connect and send GetVersion
        if let Ok(mut client) = DeviceClient::new(&port_info.port_name, 115200) {
            // Set short timeout for probing
            client.set_timeout(Duration::from_millis(500));
            if let Ok(response) = client.send_command(CommandId::GetVersion, &[]) {
                if response.resp_id == ResponseId::Version {
                    data_ports.push(port_info.port_name.clone());
                }
            }
        }
    }

    Ok(data_ports)
}

/// Find a single data port. Returns error if none found.
pub fn find_data_port() -> Result<String> {
    let ports = find_data_ports()?;
    match ports.into_iter().next() {
        Some(port) => Ok(port),
        None => anyhow::bail!("No data port found - ensure device is connected"),
    }
}

/// Find two distinct data ports for dual-device tests.
/// Returns error if fewer than two ports are found.
pub fn find_two_data_ports() -> Result<(String, String)> {
    let ports = find_data_ports()?;
    if ports.len() < 2 {
        anyhow::bail!(
            "Need at least 2 devices connected, found {}. Ports: {:?}",
            ports.len(),
            ports
        );
    }
    Ok((ports[0].clone(), ports[1].clone()))
}

/// Resolve a port argument - returns the port path if not "auto", otherwise auto-detects.
pub fn resolve_port(port_arg: &str) -> Result<String> {
    if port_arg == "auto" {
        find_data_port()
    } else {
        Ok(port_arg.to_string())
    }
}

/// Resolve two port arguments for dual-device tests.
pub fn resolve_two_ports(port_a: &str, port_b: &str) -> Result<(String, String)> {
    match (port_a, port_b) {
        ("auto", "auto") => find_two_data_ports(),
        ("auto", b) => {
            let a = find_data_port()?;
            Ok((a, b.to_string()))
        }
        (a, "auto") => {
            let b = find_data_port()?;
            Ok((a.to_string(), b))
        }
        (a, b) => Ok((a.to_string(), b.to_string())),
    }
}

/// Client for communicating with the walkie-textie device.
pub struct DeviceClient {
    port: Box<dyn SerialPort>,
    timeout: Duration,
}

impl DeviceClient {
    /// Create a new device client.
    pub fn new(port_name: &str, baud_rate: u32) -> Result<Self> {
        let port = serialport::new(port_name, baud_rate)
            .timeout(Duration::from_secs(2))
            .open()?;

        Ok(Self {
            port,
            timeout: Duration::from_secs(2),
        })
    }

    /// Set the response timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Clear any pending data in the serial buffer.
    pub fn clear_buffer(&mut self) -> Result<()> {
        self.port.clear(serialport::ClearBuffer::All)?;
        Ok(())
    }

    /// Drain all pending data from the serial port.
    /// Reads until no more data is available (with a short timeout).
    pub fn drain_buffer(&mut self) -> Result<()> {
        self.port.clear(serialport::ClearBuffer::All)?;

        // Set a very short timeout for draining
        self.port.set_timeout(Duration::from_millis(100))?;

        // Read and discard any pending data
        let mut buf = [0u8; 256];
        loop {
            match self.port.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => continue, // Keep reading
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => break,
                Err(e) => return Err(e.into()),
            }
        }

        // Restore default timeout
        self.port.set_timeout(Duration::from_secs(2))?;
        Ok(())
    }

    /// Send a command and wait for response.
    pub fn send_command(&mut self, cmd_id: CommandId, payload: &[u8]) -> Result<Response> {
        // Build and send command
        let frame = build_command(cmd_id, payload);
        self.port.write_all(&frame)?;
        self.port.flush()?;

        // Read response until zero delimiter
        let response_data = self.read_frame()?;

        // Decode and parse (add zero delimiter back - corncobs expects it)
        let mut frame_with_delimiter = response_data;
        frame_with_delimiter.push(0x00);
        let decoded = cobs_decode(&frame_with_delimiter)?;
        parse_response(&decoded)
    }

    /// Read bytes until zero delimiter.
    fn read_frame(&mut self) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        let mut buf = [0u8; 1];
        let start = Instant::now();

        while start.elapsed() < self.timeout {
            match self.port.read(&mut buf) {
                Ok(1) => {
                    if buf[0] == 0x00 {
                        if !data.is_empty() {
                            return Ok(data);
                        }
                    } else {
                        data.push(buf[0]);
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }

        anyhow::bail!(
            "Timeout waiting for response, got {} bytes: {:02x?}",
            data.len(),
            data
        );
    }

    /// Send a raw command with custom command ID (for testing invalid commands).
    pub fn send_raw_command(&mut self, cmd_id: u8, payload: &[u8]) -> Result<Response> {
        let frame = build_command_payload(cmd_id, payload);
        let encoded = cobs_encode(&frame);

        self.port.write_all(&encoded)?;
        self.port.flush()?;

        let response_data = self.read_frame()?;
        // Add zero delimiter back - corncobs expects it
        let mut frame_with_delimiter = response_data;
        frame_with_delimiter.push(0x00);
        let decoded = cobs_decode(&frame_with_delimiter)?;
        parse_response(&decoded)
    }

    /// Send LoRa TX command with data.
    pub fn lora_tx(&mut self, data: &[u8]) -> Result<Response> {
        self.send_command(CommandId::LoraTx, data)
    }

    /// Try to read an unsolicited response (non-blocking with short timeout).
    /// Returns None if no response available within timeout.
    pub fn try_read_response(&mut self, timeout: Duration) -> Result<Option<Response>> {
        let old_timeout = self.timeout;
        self.timeout = timeout;

        let result = self.read_frame();
        self.timeout = old_timeout;

        match result {
            Ok(data) => {
                let mut frame_with_delimiter = data;
                frame_with_delimiter.push(0x00);
                let decoded = cobs_decode(&frame_with_delimiter)?;
                Ok(Some(parse_response(&decoded)?))
            }
            Err(e) => {
                // Check if it's a timeout error
                let err_str = e.to_string();
                if err_str.contains("Timeout") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Wait for an unsolicited RxPacket response.
    pub fn wait_for_rx_packet(&mut self, timeout: Duration) -> Result<Response> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if let Some(response) = self.try_read_response(Duration::from_millis(100))? {
                return Ok(response);
            }
        }
        anyhow::bail!("Timeout waiting for RxPacket")
    }

    /// Clone the underlying port for use in a separate thread.
    /// Returns the port path for re-opening.
    pub fn port_name(&self) -> Result<String> {
        Ok(self.port.name().unwrap_or_default())
    }
}
