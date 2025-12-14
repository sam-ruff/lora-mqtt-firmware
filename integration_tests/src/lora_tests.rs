//! Two-device LoRa integration tests.
//!
//! Tests bidirectional LoRa communication between two devices.
//! Requires two flashed devices connected to the host.

mod device;
mod protocol;

use std::time::Duration;

use clap::Parser;
use colored::Colorize;

use device::{resolve_two_ports, DeviceClient};
use protocol::ResponseId;

#[derive(Parser)]
#[command(name = "lora-tests")]
#[command(about = "Two-device LoRa integration tests")]
struct Args {
    /// Serial port for device A (transmitter first, use "auto" to auto-detect)
    #[arg(long, default_value = "auto")]
    port_a: String,

    /// Serial port for device B (receiver first, use "auto" to auto-detect)
    #[arg(long, default_value = "auto")]
    port_b: String,

    /// Baud rate
    #[arg(short, long, default_value = "115200")]
    baud: u32,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Resolve ports (auto-detect if "auto")
    let (port_a, port_b) = resolve_two_ports(&args.port_a, &args.port_b)?;

    println!("{}", "LoRa Two-Device Integration Tests".bold());
    println!("Device A: {}", port_a);
    println!("Device B: {}", port_b);
    println!("Baud: {}", args.baud);
    println!();

    // Connect to both devices
    println!("Connecting to devices...");
    let mut device_a = DeviceClient::new(&port_a, args.baud)?;
    let mut device_b = DeviceClient::new(&port_b, args.baud)?;

    // Wait for bootloader output to finish and drain buffers
    println!("Waiting for bootloader output to finish...");
    std::thread::sleep(Duration::from_millis(500));
    device_a.drain_buffer()?;
    device_b.drain_buffer()?;
    println!("{}", "Connected to both devices!".green());

    // Verify both devices respond
    println!("\nVerifying device connectivity...");
    verify_device(&mut device_a, "A")?;
    verify_device(&mut device_b, "B")?;

    // Allow time for LoRa radios to enter RX mode after init
    // On first boot, the radio needs time to complete its initialisation
    println!("Waiting for LoRa radios to initialise...");
    std::thread::sleep(Duration::from_millis(500));

    // Clear any packets that may have been received during init
    device_a.clear_buffer()?;
    device_b.clear_buffer()?;

    println!("\n{}", "Running LoRa tests...".bold());
    println!();

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: A transmits, B receives
    print!("  Test 1: A -> B transmission ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_a_to_b(&mut device_a, &mut device_b) {
        Ok(()) => {
            println!("{}", "PASS".green().bold());
            passed += 1;
        }
        Err(e) => {
            println!("{}", "FAIL".red().bold());
            println!("    {}", e.to_string().red());
            failed += 1;
        }
    }

    // Test 2: B transmits, A receives
    print!("  Test 2: B -> A transmission ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_b_to_a(&mut device_a, &mut device_b) {
        Ok(()) => {
            println!("{}", "PASS".green().bold());
            passed += 1;
        }
        Err(e) => {
            println!("{}", "FAIL".red().bold());
            println!("    {}", e.to_string().red());
            failed += 1;
        }
    }

    // Test 3: Bidirectional ping-pong
    print!("  Test 3: Bidirectional ping-pong ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_ping_pong(&mut device_a, &mut device_b) {
        Ok(()) => {
            println!("{}", "PASS".green().bold());
            passed += 1;
        }
        Err(e) => {
            println!("{}", "FAIL".red().bold());
            println!("    {}", e.to_string().red());
            failed += 1;
        }
    }

    // Test 4: Multiple messages
    print!("  Test 4: Multiple sequential messages ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_multiple_messages(&mut device_a, &mut device_b) {
        Ok(()) => {
            println!("{}", "PASS".green().bold());
            passed += 1;
        }
        Err(e) => {
            println!("{}", "FAIL".red().bold());
            println!("    {}", e.to_string().red());
            failed += 1;
        }
    }

    // Test 5: Reliability test - 10 messages back and forth
    print!("  Test 5: Reliability (10 round trips) ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_reliability(&mut device_a, &mut device_b) {
        Ok(()) => {
            println!("{}", "PASS".green().bold());
            passed += 1;
        }
        Err(e) => {
            println!("{}", "FAIL".red().bold());
            println!("    {}", e.to_string().red());
            failed += 1;
        }
    }

    // Summary
    println!("\n{}", "=".repeat(60));
    println!("{}", "Test Results".bold());
    println!("{}", "=".repeat(60));
    println!(
        "  Total: {} passed, {} failed",
        passed.to_string().green(),
        if failed > 0 {
            failed.to_string().red()
        } else {
            failed.to_string().normal()
        }
    );
    println!("{}", "=".repeat(60));

    if failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

/// Verify a device responds to GetVersion.
fn verify_device(device: &mut DeviceClient, name: &str) -> anyhow::Result<()> {
    let response = device.send_command(protocol::CommandId::GetVersion, &[])?;
    if response.resp_id != ResponseId::Version {
        anyhow::bail!("Device {} did not respond with Version", name);
    }
    let (major, minor, patch) = (
        response.payload[0],
        response.payload[1],
        response.payload[2],
    );
    println!("  Device {}: v{}.{}.{}", name, major, minor, patch);
    Ok(())
}

/// Test: Device A transmits, Device B receives.
fn test_a_to_b(device_a: &mut DeviceClient, device_b: &mut DeviceClient) -> anyhow::Result<()> {
    let test_data = b"Hello from A!";

    // Clear any pending data on B
    device_b.clear_buffer()?;

    // A transmits
    let tx_response = device_a.lora_tx(test_data)?;
    if tx_response.resp_id != ResponseId::TxComplete {
        anyhow::bail!("Expected TxComplete, got {:?}", tx_response.resp_id);
    }

    // B should receive the packet as an unsolicited RxPacket
    let rx_response = device_b.wait_for_rx_packet(Duration::from_secs(5))?;
    if rx_response.resp_id != ResponseId::RxPacket {
        anyhow::bail!("Expected RxPacket, got {:?}", rx_response.resp_id);
    }

    // Verify payload matches (RxPacket format: [data...][rssi: i16 LE][snr: i8])
    // Payload is: data bytes, then 2 bytes RSSI, then 1 byte SNR
    if rx_response.payload.len() < test_data.len() + 3 {
        anyhow::bail!(
            "RxPacket payload too short: {} bytes (expected at least {})",
            rx_response.payload.len(),
            test_data.len() + 3
        );
    }

    let received_data = &rx_response.payload[..rx_response.payload.len() - 3];
    if received_data != test_data {
        anyhow::bail!(
            "Data mismatch: expected {:?}, got {:?}",
            test_data,
            received_data
        );
    }

    Ok(())
}

/// Test: Device B transmits, Device A receives.
fn test_b_to_a(device_a: &mut DeviceClient, device_b: &mut DeviceClient) -> anyhow::Result<()> {
    let test_data = b"Hello from B!";

    // Clear any pending data on A
    device_a.clear_buffer()?;

    // B transmits
    let tx_response = device_b.lora_tx(test_data)?;
    if tx_response.resp_id != ResponseId::TxComplete {
        anyhow::bail!("Expected TxComplete, got {:?}", tx_response.resp_id);
    }

    // A should receive the packet
    let rx_response = device_a.wait_for_rx_packet(Duration::from_secs(5))?;
    if rx_response.resp_id != ResponseId::RxPacket {
        anyhow::bail!("Expected RxPacket, got {:?}", rx_response.resp_id);
    }

    let received_data = &rx_response.payload[..rx_response.payload.len() - 3];
    if received_data != test_data {
        anyhow::bail!(
            "Data mismatch: expected {:?}, got {:?}",
            test_data,
            received_data
        );
    }

    Ok(())
}

/// Test: Bidirectional ping-pong communication.
fn test_ping_pong(device_a: &mut DeviceClient, device_b: &mut DeviceClient) -> anyhow::Result<()> {
    // Clear buffers
    device_a.clear_buffer()?;
    device_b.clear_buffer()?;

    // A sends "PING"
    let ping = b"PING";
    device_a.lora_tx(ping)?;

    // B receives PING
    let rx = device_b.wait_for_rx_packet(Duration::from_secs(5))?;
    let received = &rx.payload[..rx.payload.len() - 3];
    if received != ping {
        anyhow::bail!("B expected PING, got {:?}", received);
    }

    // Small delay to ensure A is listening
    std::thread::sleep(Duration::from_millis(200));

    // B sends "PONG"
    let pong = b"PONG";
    device_b.lora_tx(pong)?;

    // A receives PONG
    let rx = device_a.wait_for_rx_packet(Duration::from_secs(5))?;
    let received = &rx.payload[..rx.payload.len() - 3];
    if received != pong {
        anyhow::bail!("A expected PONG, got {:?}", received);
    }

    Ok(())
}

/// Test: Multiple sequential messages from A to B.
fn test_multiple_messages(
    device_a: &mut DeviceClient,
    device_b: &mut DeviceClient,
) -> anyhow::Result<()> {
    // Clear buffers
    device_a.clear_buffer()?;
    device_b.clear_buffer()?;

    let messages = [b"Message 1" as &[u8], b"Message 2", b"Message 3"];

    for (i, msg) in messages.iter().enumerate() {
        // A transmits
        let tx = device_a.lora_tx(msg)?;
        if tx.resp_id != ResponseId::TxComplete {
            anyhow::bail!("Message {} TX failed", i + 1);
        }

        // B receives
        let rx = device_b.wait_for_rx_packet(Duration::from_secs(5))?;
        let received = &rx.payload[..rx.payload.len() - 3];
        if received != *msg {
            anyhow::bail!(
                "Message {} mismatch: expected {:?}, got {:?}",
                i + 1,
                msg,
                received
            );
        }

        // Small delay between messages
        std::thread::sleep(Duration::from_millis(100));
    }

    Ok(())
}

/// Test: Reliability test with 10 round-trip messages.
fn test_reliability(
    device_a: &mut DeviceClient,
    device_b: &mut DeviceClient,
) -> anyhow::Result<()> {
    // Clear buffers
    device_a.clear_buffer()?;
    device_b.clear_buffer()?;

    for i in 0..10 {
        // A -> B
        let msg_a = format!("A->B:{:02}", i);
        let tx = device_a.lora_tx(msg_a.as_bytes())?;
        if tx.resp_id != ResponseId::TxComplete {
            anyhow::bail!("Round {} A->B TX failed", i + 1);
        }

        let rx = device_b.wait_for_rx_packet(Duration::from_secs(5))?;
        let received = &rx.payload[..rx.payload.len() - 3];
        if received != msg_a.as_bytes() {
            anyhow::bail!(
                "Round {} A->B mismatch: expected {:?}, got {:?}",
                i + 1,
                msg_a.as_bytes(),
                received
            );
        }

        // Small delay before reply
        std::thread::sleep(Duration::from_millis(50));

        // B -> A
        let msg_b = format!("B->A:{:02}", i);
        let tx = device_b.lora_tx(msg_b.as_bytes())?;
        if tx.resp_id != ResponseId::TxComplete {
            anyhow::bail!("Round {} B->A TX failed", i + 1);
        }

        let rx = device_a.wait_for_rx_packet(Duration::from_secs(5))?;
        let received = &rx.payload[..rx.payload.len() - 3];
        if received != msg_b.as_bytes() {
            anyhow::bail!(
                "Round {} B->A mismatch: expected {:?}, got {:?}",
                i + 1,
                msg_b.as_bytes(),
                received
            );
        }

        // Small delay between rounds
        std::thread::sleep(Duration::from_millis(50));
    }

    Ok(())
}
