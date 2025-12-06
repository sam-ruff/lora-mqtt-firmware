//! Device communication client.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use serialport::SerialPort;

use crate::protocol::{build_command, cobs_decode, cobs_encode, build_command_payload, parse_response, CommandId, Response};

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
