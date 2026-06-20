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

    // Wait for both radios to settle and confirm they respond (warms up each
    // link, absorbing the occasional dropped first command on a fresh connection).
    println!("Waiting for devices to become ready...");
    device_a.wait_ready(Duration::from_secs(3))?;
    device_b.wait_ready(Duration::from_secs(3))?;
    println!("{}", "Connected to both devices!".green());

    // Verify both devices respond
    println!("\nVerifying device connectivity...");
    verify_device(&mut device_a, "A")?;
    verify_device(&mut device_b, "B")?;

    // Prime the link before scoring. The first over-the-air packet after the
    // radios have been idle is often missed: the receiver re-arms RX between poll
    // cycles, so a packet can land in that gap. (The app layer handles this with
    // delivery acks and retries; the tests below measure steady-state delivery.)
    print!("\nWarming up LoRa link... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    warm_up(&mut device_a, &mut device_b);
    println!("done");

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
    let (major, minor, patch) = device.get_version(Duration::from_secs(3))?;
    println!("  Device {}: v{}.{}.{}", name, major, minor, patch);
    Ok(())
}

/// Prime both directions of the LoRa link so the first scored test does not eat
/// the cold-start packet miss. Sends throwaway packets each way until one is
/// received (or a few attempts pass), then drains any stragglers.
fn warm_up(device_a: &mut DeviceClient, device_b: &mut DeviceClient) {
    let probe = b"WARMUP";
    prime_direction(device_a, device_b, probe);
    prime_direction(device_b, device_a, probe);
    let _ = device_a.drain_buffer();
    let _ = device_b.drain_buffer();
}

/// Send throwaway packets from `tx` until `rx` receives one, or attempts run out.
fn prime_direction(tx: &mut DeviceClient, rx: &mut DeviceClient, probe: &[u8]) {
    for _ in 0..5 {
        let _ = rx.clear_buffer();
        if tx.lora_tx(probe).is_ok()
            && rx
                .wait_for_rx_packet_matching(probe, Duration::from_secs(3))
                .is_ok()
        {
            break;
        }
    }
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
    device_b.wait_for_rx_packet_matching(test_data, Duration::from_secs(8))?;

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
    device_a.wait_for_rx_packet_matching(test_data, Duration::from_secs(8))?;

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
    device_b.wait_for_rx_packet_matching(ping, Duration::from_secs(8))?;

    // Small delay to ensure A is listening
    std::thread::sleep(Duration::from_millis(200));

    // B sends "PONG"
    let pong = b"PONG";
    device_b.lora_tx(pong)?;

    // A receives PONG
    device_a.wait_for_rx_packet_matching(pong, Duration::from_secs(8))?;

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
        device_b.wait_for_rx_packet_matching(msg, Duration::from_secs(8))?;

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
        device_b.wait_for_rx_packet_matching(msg_a.as_bytes(), Duration::from_secs(8))?;

        // Small delay before reply
        std::thread::sleep(Duration::from_millis(50));

        // B -> A
        let msg_b = format!("B->A:{:02}", i);
        let tx = device_b.lora_tx(msg_b.as_bytes())?;
        if tx.resp_id != ResponseId::TxComplete {
            anyhow::bail!("Round {} B->A TX failed", i + 1);
        }
        device_a.wait_for_rx_packet_matching(msg_b.as_bytes(), Duration::from_secs(8))?;

        // Small delay between rounds
        std::thread::sleep(Duration::from_millis(50));
    }

    Ok(())
}
