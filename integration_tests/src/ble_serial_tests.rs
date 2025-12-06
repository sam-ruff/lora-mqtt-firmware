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
use device::DeviceClient;
use protocol::ResponseId;

#[derive(Parser)]
#[command(name = "ble-serial-tests")]
#[command(about = "BLE-Serial crossover integration tests for LoRa")]
struct Args {
    /// Serial port for device B
    #[arg(long, default_value = "/dev/ttyACM0")]
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

    println!("{}", "BLE-Serial Integration Tests".bold());
    println!("Device A: BLE (scanning for \"{}\")", args.ble_name);
    println!("Device B: Serial ({})", args.port_b);
    println!();

    // Connect to Device B via serial
    println!("Connecting to Device B via serial...");
    let mut device_b = DeviceClient::new(&args.port_b, args.baud)?;

    // Wait for bootloader output to finish and drain buffer
    std::thread::sleep(Duration::from_millis(500));
    device_b.drain_buffer()?;
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
        .lora_tx(test_data, Duration::from_secs(2))
        .await?;

    if tx_response.resp_id != ResponseId::TxComplete {
        anyhow::bail!("Expected TxComplete, got {:?}", tx_response.resp_id);
    }

    // B should receive the packet as an unsolicited RxPacket
    let rx_response = device_b.wait_for_rx_packet(Duration::from_secs(5))?;

    if rx_response.resp_id != ResponseId::RxPacket {
        anyhow::bail!("Expected RxPacket, got {:?}", rx_response.resp_id);
    }

    // Verify payload matches (RxPacket format: [data...][rssi: i16 LE][snr: i8])
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

    // A should receive the packet via BLE notification
    let rx_response = device_a.wait_for_rx_packet(Duration::from_secs(5)).await?;

    if rx_response.resp_id != ResponseId::RxPacket {
        anyhow::bail!("Expected RxPacket, got {:?}", rx_response.resp_id);
    }

    // Verify payload matches
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
    let tx = device_a.lora_tx(ping, Duration::from_secs(2)).await?;
    if tx.resp_id != ResponseId::TxComplete {
        anyhow::bail!("PING TX failed");
    }

    // B (Serial) receives PING
    let rx = device_b.wait_for_rx_packet(Duration::from_secs(5))?;
    let received = &rx.payload[..rx.payload.len() - 3];
    if received != ping {
        anyhow::bail!("B expected PING, got {:?}", received);
    }

    // Small delay to ensure A is listening
    tokio::time::sleep(Duration::from_millis(200)).await;

    // B (Serial) sends "PONG"
    let pong = b"PONG";
    let tx = device_b.lora_tx(pong)?;
    if tx.resp_id != ResponseId::TxComplete {
        anyhow::bail!("PONG TX failed");
    }

    // A (BLE) receives PONG via notification
    let rx = device_a.wait_for_rx_packet(Duration::from_secs(5)).await?;
    let received = &rx.payload[..rx.payload.len() - 3];
    if received != pong {
        anyhow::bail!("A (BLE) expected PONG, got {:?}", received);
    }

    Ok(())
}
