//! Duty cycle lockout test (single device).
//!
//! Deliberately spends the radio's hourly airtime budget (~6 minutes of real
//! transmission at SF11), then verifies the firmware:
//! - refuses further transmissions with a TxRefused retry hint,
//! - still accepts a spreading factor change while locked out,
//! - stays locked out at the cheaper spreading factor once the budget is dry.
//!
//! Ends by rebooting the device, which clears the in-RAM accounting and
//! restores the SF11 default.

mod device;
mod protocol;

use std::io::Write;
use std::time::Duration;

use clap::Parser;
use colored::Colorize;

use device::{resolve_port, DeviceClient};
use protocol::{CommandId, ResponseId};

/// Generous ceiling on sends: the 10% band fits ~110 max-size SF11 packets.
const MAX_SF11_SENDS: usize = 130;
/// Draining the leftover headroom at SF7 takes at most a handful of packets.
const MAX_SF7_DRAIN: usize = 25;

#[derive(Parser)]
#[command(name = "duty-cycle-tests")]
#[command(about = "Spend the duty cycle budget and verify the lockout behaviour")]
struct Args {
    /// Serial port (use "auto" to auto-detect the first device)
    #[arg(long, default_value = "auto")]
    port: String,

    /// Baud rate
    #[arg(short, long, default_value = "115200")]
    baud: u32,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let port = resolve_port(&args.port)?;

    println!("{}", "Duty Cycle Lockout Test".bold());
    println!("Device: {}", port);
    println!(
        "{}",
        "This transmits ~6 minutes of real airtime to spend the hourly budget.".yellow()
    );
    println!();

    let mut device = DeviceClient::new(&port, args.baud)?;
    device.wait_ready(Duration::from_secs(3))?;
    // A max-size SF11 packet is ~3.25 s on air; leave slack for the reply.
    device.set_timeout(Duration::from_secs(12));

    // The budget must be full for the packet counts below to mean anything.
    print!("Checking SF11 default... ");
    let sf = read_spreading_factor(&mut device)?;
    if sf != 11 {
        anyhow::bail!("Expected SF11 default, got SF{} - reboot the device first", sf);
    }
    println!("{}", "OK".green());

    // Phase 1: spend the budget with max-size packets until refused.
    println!("Spending the airtime budget at SF11 (~3.25 s per packet)...");
    let payload = [0xAA_u8; 255];
    let mut sent = 0;
    let mut retry_secs = None;
    for i in 0..MAX_SF11_SENDS {
        let response = device.lora_tx(&payload)?;
        match response.resp_id {
            ResponseId::TxComplete => {
                sent += 1;
                if sent % 10 == 0 {
                    print!("  {} packets (~{} s airtime)\r", sent, sent * 3247 / 1000);
                    std::io::stdout().flush().ok();
                }
            }
            ResponseId::TxRefused => {
                retry_secs = Some(parse_retry_secs(&response.payload)?);
                println!("\n  Refused after {} packets (send {})", sent, i + 1);
                break;
            }
            other => anyhow::bail!("Unexpected response during spend: {:?}", other),
        }
    }
    let retry_secs =
        retry_secs.ok_or_else(|| anyhow::anyhow!("No refusal after {} packets", MAX_SF11_SENDS))?;

    // ~110 packets of 3.247 s fill the 360 s allowance of the 10% band.
    if !(100..=115).contains(&sent) {
        anyhow::bail!("Refusal after {} packets; expected ~110 for the 10% band", sent);
    }
    if retry_secs == 0 || retry_secs > 3_600 {
        anyhow::bail!("Retry hint {} s is outside (0, 3600]", retry_secs);
    }
    println!(
        "  {} Lockout hit after ~{} s airtime, retry hint {} s",
        "PASS".green().bold(),
        sent * 3247 / 1000,
        retry_secs
    );

    // Phase 2: config changes must still work while TX is locked out.
    print!("Changing SF while locked out... ");
    device.set_spreading_factor(7)?;
    let sf = read_spreading_factor(&mut device)?;
    if sf != 7 {
        anyhow::bail!("SetSpreadingFactor accepted but GetRadioConfig reports SF{}", sf);
    }
    println!("{}", "PASS".green().bold());

    // Phase 3: the budget is shared across SFs. A max-size SF7 packet is only
    // ~314 ms, so a few may fit in the headroom left by the refused 3.25 s
    // packet - but the lockout must return within a handful of sends.
    println!("Draining the remaining headroom at SF7 (~314 ms per packet)...");
    let mut drained = 0;
    let mut locked_out = false;
    for _ in 0..MAX_SF7_DRAIN {
        let response = device.lora_tx(&payload)?;
        match response.resp_id {
            ResponseId::TxComplete => drained += 1,
            ResponseId::TxRefused => {
                let secs = parse_retry_secs(&response.payload)?;
                if secs == 0 || secs > 3_600 {
                    anyhow::bail!("SF7 retry hint {} s is outside (0, 3600]", secs);
                }
                println!(
                    "  {} Locked out again after {} SF7 packets, retry hint {} s",
                    "PASS".green().bold(),
                    drained,
                    secs
                );
                locked_out = true;
                break;
            }
            other => anyhow::bail!("Unexpected response during drain: {:?}", other),
        }
    }
    if !locked_out {
        anyhow::bail!("Still transmitting after {} SF7 packets; budget not shared?", MAX_SF7_DRAIN);
    }

    // An immediate resend must stay refused: refusals charge nothing and the
    // bucket window only frees whole minutes.
    print!("Confirming the lockout holds on an immediate resend... ");
    let response = device.lora_tx(&payload)?;
    if response.resp_id != ResponseId::TxRefused {
        anyhow::bail!("Expected TxRefused on resend, got {:?}", response.resp_id);
    }
    println!("{}", "PASS".green().bold());

    // Leave the board usable: a reboot clears the in-RAM accounting and
    // restores the SF11 default (the port re-enumerates). Reboot (0x03) sends
    // no reply, so drop the wait to a moment.
    println!("Rebooting the device to clear the accounting...");
    device.set_timeout(Duration::from_millis(500));
    let _ = device.send_raw_command(0x03, &[]);

    println!("\n{}", "Duty cycle lockout test passed.".green().bold());
    Ok(())
}

/// Read the active spreading factor via GetRadioConfig.
fn read_spreading_factor(device: &mut DeviceClient) -> anyhow::Result<u8> {
    let response = device.send_command(CommandId::GetRadioConfig, &[])?;
    if response.resp_id != ResponseId::RadioConfig {
        anyhow::bail!("Expected RadioConfig, got {:?}", response.resp_id);
    }
    response
        .payload
        .get(4)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("RadioConfig payload too short"))
}

/// TxRefused payload: [retry_after_secs: u32 LE].
fn parse_retry_secs(payload: &[u8]) -> anyhow::Result<u32> {
    if payload.len() != 4 {
        anyhow::bail!("Expected 4-byte TxRefused payload, got {}", payload.len());
    }
    Ok(u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]))
}
