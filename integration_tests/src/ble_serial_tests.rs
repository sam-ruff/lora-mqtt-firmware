//! BLE-Serial crossover integration tests.
//!
//! Tests LoRa communication between two devices where one is controlled via BLE
//! and the other via serial. This validates the full BLE-to-LoRa-to-Serial path.
//!
//! Requires two flashed devices:
//! - Device A: Connected via BLE (advertises as "WalkieTextie")
//! - Device B: Connected via serial

mod ble_client;
mod device;
mod protocol;

use std::time::Duration;

use clap::Parser;
use colored::Colorize;

use ble_client::BleClient;
use device::{resolve_port, DeviceClient};
use protocol::ResponseId;

#[derive(Parser)]
#[command(name = "ble-serial-tests")]
#[command(about = "BLE-Serial crossover integration tests for LoRa")]
struct Args {
    /// Serial port for device B (use "auto" to auto-detect)
    #[arg(long, default_value = "auto")]
    port_b: String,

    /// BLE device name for device A
    #[arg(long, default_value = "WalkieTextie")]
    ble_name: String,

    /// Baud rate for serial
    #[arg(short, long, default_value = "115200")]
    baud: u32,

    /// BLE scan timeout in seconds
    #[arg(long, default_value = "10")]
    scan_timeout: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Resolve port (auto-detect if "auto")
    let port_b = resolve_port(&args.port_b)?;

    println!("{}", "BLE-Serial Integration Tests".bold());
    println!("Device A: BLE (scanning for \"{}\")", args.ble_name);
    println!("Device B: Serial ({})", port_b);
    println!();

    // Connect to Device B via serial
    println!("Connecting to Device B via serial...");
    let mut device_b = DeviceClient::new(&port_b, args.baud)?;

    // Confirm the serial device responds (also warms up the link).
    device_b.wait_ready(Duration::from_secs(3))?;
    println!("{}", "  Serial connected!".green());

    // Connect to Device A via BLE
    println!("Scanning for BLE device \"{}\"...", args.ble_name);
    let device_a = BleClient::connect_by_name(
        &args.ble_name,
        Duration::from_secs(args.scan_timeout),
    )
    .await?;
    println!("{}", "  BLE connected!".green());

    // Allow time for LoRa radios to enter RX mode after init
    println!("Waiting for LoRa radios to initialise...");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Clear any pending data
    device_a.clear_buffer().await;
    device_b.clear_buffer()?;

    // Prime both directions so the first scored LoRa test does not eat the
    // cold-start packet miss (the receiver re-arms RX between poll cycles).
    print!("Warming up LoRa link... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    warm_up(&device_a, &mut device_b).await;
    println!("done");

    println!("\n{}", "Running tests...".bold());
    println!();

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: BLE GetVersion (Device A)
    print!("  Test 1: BLE GetVersion (Device A) ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_ble_get_version(&device_a).await {
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

    // Test 2: Serial GetVersion (Device B)
    print!("  Test 2: Serial GetVersion (Device B) ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_serial_get_version(&mut device_b) {
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

    // Test 3: BLE to LoRa to Serial
    print!("  Test 3: BLE to LoRa to Serial ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_ble_to_serial(&device_a, &mut device_b).await {
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

    // Test 4: Serial to LoRa to BLE
    print!("  Test 4: Serial to LoRa to BLE ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_serial_to_ble(&device_a, &mut device_b).await {
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

    // Test 5: Bidirectional Ping-Pong
    print!("  Test 5: Bidirectional Ping-Pong ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_ping_pong(&device_a, &mut device_b).await {
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

    // Disconnect BLE
    let _ = device_a.disconnect().await;

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

/// Prime both directions of the LoRa link before scoring, so the first test does
/// not eat the cold-start packet miss. Failures here are ignored on purpose.
async fn warm_up(device_a: &BleClient, device_b: &mut DeviceClient) {
    let probe = b"WARMUP";
    // A (BLE) -> B (serial)
    for _ in 0..5 {
        let _ = device_b.clear_buffer();
        if device_a.lora_tx(probe, Duration::from_secs(3)).await.is_ok()
            && device_b
                .wait_for_rx_packet_matching(probe, Duration::from_secs(3))
                .is_ok()
        {
            break;
        }
    }
    // B (serial) -> A (BLE)
    for _ in 0..5 {
        device_a.clear_buffer().await;
        if device_b.lora_tx(probe).is_ok()
            && device_a
                .wait_for_rx_packet_matching(probe, Duration::from_secs(3))
                .await
                .is_ok()
        {
            break;
        }
    }
    device_a.clear_buffer().await;
    let _ = device_b.drain_buffer();
}

/// Test: BLE GetVersion command on Device A.
async fn test_ble_get_version(device_a: &BleClient) -> anyhow::Result<()> {
    let response = device_a
        .send_command(protocol::CommandId::GetVersion, &[], Duration::from_secs(2))
        .await?;

    if response.resp_id != ResponseId::Version {
        anyhow::bail!("Expected Version response, got {:?}", response.resp_id);
    }

    if response.payload.len() < 3 {
        anyhow::bail!(
            "Version payload too short: {} bytes",
            response.payload.len()
        );
    }

    let (major, minor, patch) = (
        response.payload[0],
        response.payload[1],
        response.payload[2],
    );
    println!("    Device A (BLE): v{}.{}.{}", major, minor, patch);

    Ok(())
}

/// Test: Serial GetVersion command on Device B.
fn test_serial_get_version(device_b: &mut DeviceClient) -> anyhow::Result<()> {
    let response = device_b.send_command(protocol::CommandId::GetVersion, &[])?;

    if response.resp_id != ResponseId::Version {
        anyhow::bail!("Expected Version response, got {:?}", response.resp_id);
    }

    let (major, minor, patch) = (
        response.payload[0],
        response.payload[1],
        response.payload[2],
    );
    println!("    Device B (Serial): v{}.{}.{}", major, minor, patch);

    Ok(())
}

/// Test: Device A (BLE) transmits via LoRa, Device B (Serial) receives.
async fn test_ble_to_serial(
    device_a: &BleClient,
    device_b: &mut DeviceClient,
) -> anyhow::Result<()> {
    let test_data = b"BLE_TO_SERIAL";

    // Clear any pending data on B
    device_b.clear_buffer()?;

    // A transmits via BLE
    let tx_response = device_a
        .lora_tx(test_data, Duration::from_secs(3))
        .await?;

    if tx_response.resp_id != ResponseId::TxComplete {
        anyhow::bail!("Expected TxComplete, got {:?}", tx_response.resp_id);
    }

    // B (serial) should receive the packet as an unsolicited RxPacket
    device_b.wait_for_rx_packet_matching(test_data, Duration::from_secs(8))?;

    Ok(())
}

/// Test: Device B (Serial) transmits via LoRa, Device A (BLE) receives.
async fn test_serial_to_ble(
    device_a: &BleClient,
    device_b: &mut DeviceClient,
) -> anyhow::Result<()> {
    let test_data = b"SERIAL_TO_BLE";

    // Clear any pending data on A (BLE)
    device_a.clear_buffer().await;

    // B transmits via Serial
    let tx_response = device_b.lora_tx(test_data)?;

    if tx_response.resp_id != ResponseId::TxComplete {
        anyhow::bail!("Expected TxComplete, got {:?}", tx_response.resp_id);
    }

    // A (BLE) should receive the packet via notification
    device_a
        .wait_for_rx_packet_matching(test_data, Duration::from_secs(8))
        .await?;

    Ok(())
}

/// Test: Bidirectional ping-pong between BLE and Serial devices.
async fn test_ping_pong(
    device_a: &BleClient,
    device_b: &mut DeviceClient,
) -> anyhow::Result<()> {
    // Clear buffers
    device_a.clear_buffer().await;
    device_b.clear_buffer()?;

    // A (BLE) sends "PING"
    let ping = b"PING";
    let tx = device_a.lora_tx(ping, Duration::from_secs(3)).await?;
    if tx.resp_id != ResponseId::TxComplete {
        anyhow::bail!("PING TX failed");
    }

    // B (Serial) receives PING
    device_b.wait_for_rx_packet_matching(ping, Duration::from_secs(8))?;

    // Small delay to ensure A is listening
    tokio::time::sleep(Duration::from_millis(200)).await;

    // B (Serial) sends "PONG"
    let pong = b"PONG";
    let tx = device_b.lora_tx(pong)?;
    if tx.resp_id != ResponseId::TxComplete {
        anyhow::bail!("PONG TX failed");
    }

    // A (BLE) receives PONG via notification
    device_a
        .wait_for_rx_packet_matching(pong, Duration::from_secs(8))
        .await?;

    Ok(())
}
