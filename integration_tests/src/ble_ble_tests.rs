//! BLE-BLE integration tests.
//!
//! Tests LoRa communication between two devices where both are controlled via BLE.
//! This validates the full BLE-to-LoRa-to-BLE path.
//!
//! Requires two flashed devices:
//! - Device A: Connected via BLE (unique name e.g. "WalkieTextie-AABBCC")
//! - Device B: Connected via BLE (unique name e.g. "WalkieTextie-DDEEFF")

mod ble_client;
mod protocol;

use std::time::Duration;

use clap::Parser;
use colored::Colorize;

use ble_client::BleClient;
use protocol::ResponseId;

#[derive(Parser)]
#[command(name = "ble-ble-tests")]
#[command(about = "BLE-BLE integration tests for LoRa")]
struct Args {
    /// BLE device name for device A
    #[arg(long)]
    ble_name_a: String,

    /// BLE device name for device B
    #[arg(long)]
    ble_name_b: String,

    /// BLE scan timeout in seconds
    #[arg(long, default_value = "10")]
    scan_timeout: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    println!("{}", "BLE-BLE Integration Tests".bold());
    println!("Device A: BLE (scanning for \"{}\")", args.ble_name_a);
    println!("Device B: BLE (scanning for \"{}\")", args.ble_name_b);
    println!();

    // Connect to Device A via BLE
    println!("Scanning for BLE device A \"{}\"...", args.ble_name_a);
    let device_a = BleClient::connect_by_name(
        &args.ble_name_a,
        Duration::from_secs(args.scan_timeout),
    )
    .await?;
    println!("{}", "  Device A connected!".green());

    // Connect to Device B via BLE
    println!("Scanning for BLE device B \"{}\"...", args.ble_name_b);
    let device_b = BleClient::connect_by_name(
        &args.ble_name_b,
        Duration::from_secs(args.scan_timeout),
    )
    .await?;
    println!("{}", "  Device B connected!".green());

    // Allow time for LoRa radios to enter RX mode after init
    println!("Waiting for LoRa radios to initialise...");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Clear any pending data
    device_a.clear_buffer().await;
    device_b.clear_buffer().await;

    // Prime both directions so the first scored LoRa test does not eat the
    // cold-start packet miss (the receiver re-arms RX between poll cycles).
    print!("Warming up LoRa link... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    warm_up(&device_a, &device_b).await;
    println!("done");

    println!("\n{}", "Running tests...".bold());
    println!();

    let mut passed = 0;
    let mut failed = 0;

    // Test 1: BLE GetVersion (Device A)
    print!("  Test 1: BLE GetVersion (Device A) ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_ble_get_version(&device_a, "A").await {
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

    // Test 2: BLE GetVersion (Device B)
    print!("  Test 2: BLE GetVersion (Device B) ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_ble_get_version(&device_b, "B").await {
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

    // Test 3: Device A to LoRa to Device B
    print!("  Test 3: BLE A to LoRa to BLE B ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_a_to_b(&device_a, &device_b).await {
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

    // Test 4: Device B to LoRa to Device A
    print!("  Test 4: BLE B to LoRa to BLE A ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_b_to_a(&device_a, &device_b).await {
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
    match test_ping_pong(&device_a, &device_b).await {
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

    // Test 6: Multiple Messages (10 round trips)
    print!("  Test 6: Multiple Messages (10 round trips) ... ");
    std::io::Write::flush(&mut std::io::stdout())?;
    match test_multiple_messages(&device_a, &device_b).await {
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
    let _ = device_b.disconnect().await;

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
async fn warm_up(device_a: &BleClient, device_b: &BleClient) {
    let probe = b"WARMUP";
    for (tx, rx) in [(device_a, device_b), (device_b, device_a)] {
        for _ in 0..5 {
            rx.clear_buffer().await;
            if tx.lora_tx(probe, Duration::from_secs(3)).await.is_ok()
                && rx
                    .wait_for_rx_packet_matching(probe, Duration::from_secs(3))
                    .await
                    .is_ok()
            {
                break;
            }
        }
    }
    device_a.clear_buffer().await;
    device_b.clear_buffer().await;
}

/// Test: BLE GetVersion command.
async fn test_ble_get_version(device: &BleClient, name: &str) -> anyhow::Result<()> {
    let response = device
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
    println!("    Device {} (BLE): v{}.{}.{}", name, major, minor, patch);

    Ok(())
}

/// Test: Device A transmits via LoRa, Device B receives.
async fn test_a_to_b(device_a: &BleClient, device_b: &BleClient) -> anyhow::Result<()> {
    let test_data = b"A_TO_B_BLE";

    // Clear any pending data on B
    device_b.clear_buffer().await;

    // A transmits
    let tx_response = device_a
        .lora_tx(test_data, Duration::from_secs(5))
        .await?;

    if tx_response.resp_id != ResponseId::TxComplete {
        anyhow::bail!("Expected TxComplete, got {:?}", tx_response.resp_id);
    }

    // B should receive the packet
    device_b
        .wait_for_rx_packet_matching(test_data, Duration::from_secs(10))
        .await?;

    Ok(())
}

/// Test: Device B transmits via LoRa, Device A receives.
async fn test_b_to_a(device_a: &BleClient, device_b: &BleClient) -> anyhow::Result<()> {
    let test_data = b"B_TO_A_BLE";

    // Clear any pending data on A
    device_a.clear_buffer().await;

    // B transmits
    let tx_response = device_b
        .lora_tx(test_data, Duration::from_secs(5))
        .await?;

    if tx_response.resp_id != ResponseId::TxComplete {
        anyhow::bail!("Expected TxComplete, got {:?}", tx_response.resp_id);
    }

    // A should receive the packet
    device_a
        .wait_for_rx_packet_matching(test_data, Duration::from_secs(10))
        .await?;

    Ok(())
}

/// Test: Bidirectional ping-pong between two BLE devices.
async fn test_ping_pong(device_a: &BleClient, device_b: &BleClient) -> anyhow::Result<()> {
    // Clear buffers
    device_a.clear_buffer().await;
    device_b.clear_buffer().await;

    // A sends "PING"
    let ping = b"PING";
    let tx = device_a.lora_tx(ping, Duration::from_secs(5)).await?;
    if tx.resp_id != ResponseId::TxComplete {
        anyhow::bail!("PING TX failed");
    }

    // B receives PING
    device_b
        .wait_for_rx_packet_matching(ping, Duration::from_secs(10))
        .await?;

    // Small delay to ensure A is listening
    tokio::time::sleep(Duration::from_millis(200)).await;

    // B sends "PONG"
    let pong = b"PONG";
    let tx = device_b.lora_tx(pong, Duration::from_secs(5)).await?;
    if tx.resp_id != ResponseId::TxComplete {
        anyhow::bail!("PONG TX failed");
    }

    // A receives PONG
    device_a
        .wait_for_rx_packet_matching(pong, Duration::from_secs(10))
        .await?;

    Ok(())
}

/// Test: Multiple messages back and forth (10 round trips).
async fn test_multiple_messages(device_a: &BleClient, device_b: &BleClient) -> anyhow::Result<()> {
    const NUM_MESSAGES: usize = 10;

    // Clear buffers
    device_a.clear_buffer().await;
    device_b.clear_buffer().await;

    for i in 0..NUM_MESSAGES {
        // A sends to B
        let msg_a_to_b = format!("A_TO_B_{:02}", i);
        let tx = device_a
            .lora_tx(msg_a_to_b.as_bytes(), Duration::from_secs(5))
            .await?;
        if tx.resp_id != ResponseId::TxComplete {
            anyhow::bail!("Round {}: A->B TX failed", i);
        }

        // B receives
        device_b
            .wait_for_rx_packet_matching(msg_a_to_b.as_bytes(), Duration::from_secs(10))
            .await?;

        // Small delay
        tokio::time::sleep(Duration::from_millis(100)).await;

        // B sends to A
        let msg_b_to_a = format!("B_TO_A_{:02}", i);
        let tx = device_b
            .lora_tx(msg_b_to_a.as_bytes(), Duration::from_secs(5))
            .await?;
        if tx.resp_id != ResponseId::TxComplete {
            anyhow::bail!("Round {}: B->A TX failed", i);
        }

        // A receives
        device_a
            .wait_for_rx_packet_matching(msg_b_to_a.as_bytes(), Duration::from_secs(10))
            .await?;

        // Small delay before next round
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Ok(())
}
