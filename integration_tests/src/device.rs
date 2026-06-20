//! Device communication client.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use serialport::{SerialPort, SerialPortType};

use crate::protocol::{build_command, cobs_decode, cobs_encode, build_command_payload, parse_response, CommandId, Response, ResponseId};

/// USB vendor/product id of the Walkie-Textie firmware (dual CDC-ACM device).
const USB_VID: u16 = 0x303A;
const USB_PID: u16 = 0x1001;

/// Find the live data port of every connected board.
///
/// The firmware exposes two CDC-ACM functions in a fixed order: the data port is
/// the first function (USB interface 0) and the debug/log port is the second
/// (interface 2). The kernel numbers `/dev/ttyACMN` in enumeration order, not by
/// interface role, so the data port is identified by its USB interface number,
/// never by the port number being even or odd (which is not deterministic - two
/// boards' data ports can both enumerate before either debug port).
pub fn find_data_ports() -> Result<Vec<String>> {
    let ports = serialport::available_ports()?;

    let mut data_ports = Vec::new();
    for port_info in ports {
        let SerialPortType::UsbPort(usb) = &port_info.port_type else {
            continue;
        };
        if usb.vid != USB_VID || usb.pid != USB_PID {
            continue;
        }
        // Interface 0 is the data CDC; interface 2 is the debug/log CDC.
        if usb.interface != Some(0) {
            continue;
        }

        // Confirm the firmware is actually responding before claiming the port.
        let responds = DeviceClient::new(&port_info.port_name, 115200)
            .map(|mut client| client.wait_ready(Duration::from_secs(2)).is_ok())
            .unwrap_or(false);
        if responds {
            data_ports.push(port_info.port_name.clone());
        } else {
            eprintln!(
                "  warning: {} ({}) is a data port but did not respond",
                port_info.port_name,
                usb.serial_number.as_deref().unwrap_or("unknown")
            );
        }
    }

    data_ports.sort();
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
        // The firmware can take its full RX poll interval (~500ms) plus the LoRa
        // time-on-air to answer, so keep a generous default response window.
        let timeout = Duration::from_secs(3);
        let port = serialport::new(port_name, baud_rate)
            .timeout(timeout)
            .open()?;

        Ok(Self { port, timeout })
    }

    /// Wait until the firmware answers GetVersion, or the timeout elapses.
    ///
    /// Drains any stale bytes, then retries with a short per-attempt timeout so
    /// the occasional dropped first command after a fresh CDC connection does not
    /// fail the caller. Doubles as a warm-up for the link.
    pub fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        self.get_version(timeout).map(|_| ())
    }

    /// Retry GetVersion until it answers, returning (major, minor, patch).
    ///
    /// Tolerant of the firmware's command latency (it polls LoRa RX for up to
    /// ~500ms between command checks) and of the occasional dropped command on a
    /// fresh CDC connection. Each attempt clears stale input first, so a retry
    /// never leaves a stray response behind.
    pub fn get_version(&mut self, timeout: Duration) -> Result<(u8, u8, u8)> {
        // Absorb any late reply left over from a previous session: on reconnect
        // the firmware can flush a response it was blocked on once DTR re-asserts.
        let _ = self.drain_buffer();

        let previous = self.timeout;
        let attempt = timeout.min(Duration::from_millis(1500));
        self.set_timeout(attempt);

        let start = Instant::now();
        let mut last_err = anyhow::anyhow!("device did not respond to GetVersion");
        loop {
            match self.send_command(CommandId::GetVersion, &[]) {
                Ok(resp) if resp.resp_id == ResponseId::Version && resp.payload.len() >= 3 => {
                    let version = (resp.payload[0], resp.payload[1], resp.payload[2]);
                    self.set_timeout(previous);
                    return Ok(version);
                }
                Ok(resp) => last_err = anyhow::anyhow!("unexpected response {:?}", resp.resp_id),
                Err(e) => last_err = e,
            }
            if start.elapsed() >= timeout {
                self.set_timeout(previous);
                return Err(last_err);
            }
        }
    }

    /// Set the response timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
        // Also update the serial port's read timeout
        let _ = self.port.set_timeout(timeout);
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

    /// Send a command and wait for its reply.
    pub fn send_command(&mut self, cmd_id: CommandId, payload: &[u8]) -> Result<Response> {
        let frame = build_command(cmd_id, payload);
        self.port.write_all(&frame)?;
        self.port.flush()?;
        self.read_command_response_resync()
    }

    /// Read the command reply, draining the buffer on failure so a timed-out or
    /// corrupt exchange cannot leave a partial frame that desyncs the next one.
    fn read_command_response_resync(&mut self) -> Result<Response> {
        let result = self.read_command_response();
        if result.is_err() {
            let _ = self.drain_buffer();
        }
        result
    }

    /// Read frames until the command reply arrives, skipping unsolicited packets.
    ///
    /// The device shares one stream for command replies and unsolicited LoRa
    /// RxPackets, and the slow radio can deliver a packet from an earlier
    /// exchange late. Reading whole frames (never clearing mid-frame, which would
    /// split one) and skipping RxPackets keeps the strict request/response model
    /// in sync. Command replies are Version / TxComplete / Error, never RxPacket.
    fn read_command_response(&mut self) -> Result<Response> {
        loop {
            let mut frame = self.read_frame()?;
            frame.push(0x00); // corncobs expects the delimiter
            let decoded = cobs_decode(&frame)?;
            let response = parse_response(&decoded)?;
            if response.resp_id == ResponseId::RxPacket {
                continue; // unsolicited - not the reply to our command
            }
            return Ok(response);
        }
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

        self.read_command_response_resync()
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

    /// Wait for an RxPacket whose data payload equals `expected`.
    ///
    /// The radio is slow (SF11), so a packet from an earlier exchange can be
    /// delivered late and land in the next read; skipping non-matching frames
    /// keeps the strict per-message tests in sync without masking real loss (a
    /// missing packet still times out). The trailing 3 bytes are rssi+snr.
    pub fn wait_for_rx_packet_matching(
        &mut self,
        expected: &[u8],
        timeout: Duration,
    ) -> Result<Response> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            let Some(response) = self.try_read_response(Duration::from_millis(200))? else {
                continue;
            };
            if response.resp_id != ResponseId::RxPacket || response.payload.len() < 3 {
                continue;
            }
            if &response.payload[..response.payload.len() - 3] == expected {
                return Ok(response);
            }
            // Stale or unexpected packet - keep waiting for the one we want.
        }
        anyhow::bail!("Timeout waiting for RxPacket matching {:?}", expected)
    }

    /// Clone the underlying port for use in a separate thread.
    /// Returns the port path for re-opening.
    pub fn port_name(&self) -> Result<String> {
        Ok(self.port.name().unwrap_or_default())
    }
}
