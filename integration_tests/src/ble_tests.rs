//! Single-device BLE integration tests.
//!
//! Exercises the BLE host-link path the phone app uses: scan for the device,
//! connect over the Nordic UART Service, and run the same command checks as the
//! serial tests (GetVersion, invalid command, repeated calls).
//!
//! Requires one flashed device advertising as "WalkieTextie...".

mod ble_client;
mod protocol;

use std::time::Duration;

use clap::Parser;
use colored::Colorize;

use ble_client::BleClient;
use protocol::{CommandId, ResponseId, ResponseStatus};

#[derive(Parser)]
#[command(name = "ble-tests")]
#[command(about = "Single-device BLE integration tests for walkie-textie firmware")]
struct Args {
    /// BLE device name (prefix) to scan for.
    #[arg(long, default_value = "WalkieTextie")]
    ble_name: String,

    /// BLE scan timeout in seconds.
    #[arg(long, default_value = "15")]
    scan_timeout: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    println!("{}", "Walkie-Textie BLE Integration Tests".bold());
    println!("Scanning for \"{}\" ...", args.ble_name);

    let client = BleClient::connect_by_name(&args.ble_name, Duration::from_secs(args.scan_timeout))
        .await?;
    println!("{}", "Connected!".green());
    println!("\nRunning tests...\n");

    let mut passed = 0usize;
    let mut failed = 0usize;
    for (name, result) in [
        ("GetVersion returns version bytes", test_get_version(&client).await),
        ("Invalid command returns error", test_invalid_command(&client).await),
        ("Multiple GetVersion calls succeed", test_multiple_get_version(&client).await),
    ] {
        match result {
            Ok(()) => {
                println!("  {} ... {}", name, "PASS".green());
                passed += 1;
            }
            Err(e) => {
                println!("  {} ... {} ({e})", name, "FAIL".red());
                failed += 1;
            }
        }
    }

    println!("\n{}", "=".repeat(60));
    println!("  Total: {passed} passed, {failed} failed");
    println!("{}", "=".repeat(60));

    let _ = client.disconnect().await;
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

async fn test_get_version(client: &BleClient) -> anyhow::Result<()> {
    let response = client
        .send_command(CommandId::GetVersion, &[], Duration::from_secs(2))
        .await?;
    if response.resp_id != ResponseId::Version {
        anyhow::bail!("expected Version response, got {:?}", response.resp_id);
    }
    if response.payload.len() != 3 {
        anyhow::bail!("expected 3 version bytes, got {}", response.payload.len());
    }
    println!(
        "    (v{}.{}.{})",
        response.payload[0], response.payload[1], response.payload[2]
    );
    Ok(())
}

async fn test_invalid_command(client: &BleClient) -> anyhow::Result<()> {
    // 0xFE is not a valid command id (0xFF is the Error response id).
    let response = client
        .send_raw_command(0xFE, &[], Duration::from_secs(2))
        .await?;
    if response.resp_id != ResponseId::Error {
        anyhow::bail!("expected Error response, got {:?}", response.resp_id);
    }
    let status = *response
        .payload
        .first()
        .ok_or_else(|| anyhow::anyhow!("error response payload too short"))?;
    if status != ResponseStatus::InvalidCommand as u8 {
        anyhow::bail!("expected InvalidCommand status, got 0x{status:02x}");
    }
    Ok(())
}

async fn test_multiple_get_version(client: &BleClient) -> anyhow::Result<()> {
    for _ in 0..5 {
        test_get_version(client).await?;
    }
    Ok(())
}
