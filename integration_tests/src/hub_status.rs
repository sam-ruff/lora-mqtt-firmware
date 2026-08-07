//! Print the hub's live status over USB serial - a quick diagnostic.
//!
//! Run: cargo hub-status

mod device;
mod protocol;

use std::time::Duration;

use anyhow::Result;
use clap::Parser;

use device::{resolve_port_with_prefix, DeviceClient};
use protocol::{parse_hub_status, HubCommandId, ResponseId};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "auto")]
    hub_port: String,
    /// Erase the stored network config and reboot (back to the provisioning
    /// hotspot)
    #[arg(long)]
    clear: bool,
    /// Switch the operating mode ("bridge" or "gateway") and reboot
    #[arg(long)]
    mode: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let port = resolve_port_with_prefix(&args.hub_port, "LMH-")?;
    let mut hub = DeviceClient::new(&port, 115200)?;
    hub.wait_ready(Duration::from_secs(5))?;

    let version = hub.get_version(Duration::from_secs(3))?;
    println!("hub {port} firmware v{}.{}.{}", version.0, version.1, version.2);

    // Mode switches and factory resets run before the read-out so they work
    // even when a longer response is misbehaving.
    if let Some(mode) = args.mode.as_deref() {
        let byte = match mode {
            "bridge" => 0u8,
            "gateway" => 1u8,
            other => anyhow::bail!("mode must be bridge or gateway, got {other:?}"),
        };
        hub.expect_config_ack(HubCommandId::SetMode, &[byte])?;
        hub.reboot()?;
        println!("mode set to {mode}, hub rebooting");
        return Ok(());
    }

    let response = hub.send_hub_command(HubCommandId::GetHubStatus, &[])?;
    anyhow::ensure!(
        response.resp_id == ResponseId::HubStatus,
        "unexpected response {:?}",
        response.resp_id
    );
    let status = parse_hub_status(&response.payload)?;
    println!(
        "wifi {} mqtt {} ip {}.{}.{}.{} up {} down {} dropped {} uptime {}s",
        status.wifi_state,
        status.mqtt_state,
        status.ip[0],
        status.ip[1],
        status.ip[2],
        status.ip[3],
        status.uplink_count,
        status.downlink_count,
        status.dropped_count,
        status.uptime_secs
    );

    let network = hub.send_hub_command(HubCommandId::GetNetworkConfig, &[])?;
    println!("network config payload: {:02x?}", network.payload);

    if args.clear {
        hub.expect_config_ack(HubCommandId::ClearNetworkConfig, &[])?;
        hub.reboot()?;
        println!("config cleared, hub rebooting into the provisioning hotspot");
    }
    Ok(())
}
