//! Hub bridge-mode end-to-end test.
//!
//! Needs: one hub board (USB serial WTH-...), one node board (WT-...), and a
//! reachable MQTT broker (e.g. `mosquitto` on this machine). Provisions the
//! hub over USB serial, reboots it into the new config, then proves the full
//! LoRa <-> MQTT path in both directions.
//!
//! Run: cargo hub -- --wifi-ssid <ssid> --wifi-password <pw> --broker-host <ip>

mod device;
mod protocol;

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};

use device::{resolve_port_with_prefix, DeviceClient};
use protocol::{
    link_state, mqtt_config_payload, parse_hub_status, wifi_config_payload, HubCommandId,
    ResponseId,
};

#[derive(Parser)]
struct Args {
    /// Hub board data port ("auto" detects by the WTH- USB serial)
    #[arg(long, default_value = "auto")]
    hub_port: String,
    /// Node board data port ("auto" detects by the WT- USB serial)
    #[arg(long, default_value = "auto")]
    node_port: String,
    /// WiFi SSID to provision (defaults to $HUB_WIFI_SSID)
    #[arg(long, env = "HUB_WIFI_SSID")]
    wifi_ssid: String,
    /// WiFi password to provision (defaults to $HUB_WIFI_PASSWORD)
    #[arg(long, env = "HUB_WIFI_PASSWORD", default_value = "")]
    wifi_password: String,
    /// Broker address as the hub reaches it (this machine's LAN IP)
    #[arg(long, env = "HUB_MQTT_HOST")]
    broker_host: String,
    #[arg(long, default_value_t = 1883)]
    broker_port: u16,
    /// Skip provisioning and reboot (hub already configured)
    #[arg(long)]
    skip_provision: bool,
}

fn main() {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => println!("{}", "All hub tests passed".green().bold()),
        Err(err) => {
            eprintln!("{} {err:#}", "FAILED:".red().bold());
            std::process::exit(1);
        }
    }
}

fn run(args: &Args) -> Result<()> {
    println!("{}", "Hub bridge-mode end-to-end test".bold());

    let hub_port = resolve_port_with_prefix(&args.hub_port, "WTH-")?;
    let node_port = resolve_port_with_prefix(&args.node_port, "WT-")?;
    println!("  hub:  {hub_port}\n  node: {node_port}");

    let mut hub = DeviceClient::new(&hub_port, 115200)?;
    hub.wait_ready(Duration::from_secs(5))?;

    if !args.skip_provision {
        provision(&mut hub, args)?;
        println!("  provisioned; rebooting hub");
        hub.reboot()?;
        drop(hub);
        std::thread::sleep(Duration::from_secs(3));
        let hub_port = resolve_port_with_prefix("auto", "WTH-")
            .context("hub did not re-enumerate after reboot")?;
        hub = DeviceClient::new(&hub_port, 115200)?;
        hub.wait_ready(Duration::from_secs(10))?;
    }

    let hub_id = wait_online(&mut hub)?;
    println!("  hub online, device id {hub_id}");

    // Broker side: subscribe before stimulating traffic.
    let mut options = MqttOptions::new("hub-integration-test", &args.broker_host, args.broker_port);
    options.set_keep_alive(Duration::from_secs(10));
    let (client, mut connection) = Client::new(options, 32);
    client.subscribe(format!("wt/hub/{hub_id}/#"), QoS::AtLeastOnce)?;

    // Retained status must announce the hub as online.
    let status = wait_for_message(&mut connection, &format!("wt/hub/{hub_id}/status"), 10)?;
    anyhow::ensure!(
        status.contains("\"online\":true"),
        "status topic should be online, got {status}"
    );
    println!("  {} retained online status", "ok".green());

    // Uplink: node transmits, the broker must see the JSON.
    let mut node = DeviceClient::new(&node_port, 115200)?;
    node.wait_ready(Duration::from_secs(5))?;
    let ping = b"hub-test-ping";
    let response = node.lora_tx(ping)?;
    anyhow::ensure!(
        response.resp_id == ResponseId::TxComplete,
        "node TX failed: {:?}",
        response.resp_id
    );
    let rx = wait_for_message(&mut connection, &format!("wt/hub/{hub_id}/rx"), 30)?;
    let ping_hex = hex(ping);
    anyhow::ensure!(
        rx.contains(&format!("\"payload_hex\":\"{ping_hex}\"")),
        "rx JSON missing payload: {rx}"
    );
    anyhow::ensure!(rx.contains("\"rssi\":"), "rx JSON missing rssi: {rx}");
    println!("  {} LoRa -> MQTT uplink", "ok".green());

    // Downlink: publish to the tx topic, the node must hear it over the air.
    let pong = b"hub-test-pong";
    let downlink = format!(r#"{{"payload_hex":"{}","id":7}}"#, hex(pong));
    client.publish(
        format!("wt/hub/{hub_id}/tx"),
        QoS::AtLeastOnce,
        false,
        downlink.as_bytes(),
    )?;
    node.wait_for_rx_packet_matching(pong, Duration::from_secs(30))?;
    let result = wait_for_message(&mut connection, &format!("wt/hub/{hub_id}/tx/result"), 10)?;
    anyhow::ensure!(
        result.contains("\"result\":\"sent\"") && result.contains("\"id\":7"),
        "unexpected tx result: {result}"
    );
    println!("  {} MQTT -> LoRa downlink with result", "ok".green());

    // Config echo must never contain the WiFi password.
    let network = hub.send_hub_command(HubCommandId::GetNetworkConfig, &[])?;
    anyhow::ensure!(
        network.resp_id == ResponseId::NetworkConfig,
        "expected NetworkConfig, got {:?}",
        network.resp_id
    );
    if !args.wifi_password.is_empty() {
        let payload = network.payload;
        anyhow::ensure!(
            !payload
                .windows(args.wifi_password.len())
                .any(|w| w == args.wifi_password.as_bytes()),
            "NetworkConfig leaked the WiFi password"
        );
    }
    println!("  {} config echo redacts the password", "ok".green());

    Ok(())
}

fn provision(hub: &mut DeviceClient, args: &Args) -> Result<()> {
    hub.expect_config_ack(
        HubCommandId::SetWifiConfig,
        &wifi_config_payload(&args.wifi_ssid, &args.wifi_password),
    )
    .context("SetWifiConfig")?;
    hub.expect_config_ack(
        HubCommandId::SetMqttConfig,
        &mqtt_config_payload(&args.broker_host, args.broker_port, ""),
    )
    .context("SetMqttConfig")?;
    // Bridge mode.
    hub.expect_config_ack(HubCommandId::SetMode, &[0]).context("SetMode")?;
    Ok(())
}

/// Poll GetHubStatus until WiFi and MQTT report connected; returns the hub id
/// (from the USB serial) for topic construction.
fn wait_online(hub: &mut DeviceClient) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let response = hub.send_hub_command(HubCommandId::GetHubStatus, &[])?;
        if response.resp_id == ResponseId::HubStatus {
            let status = parse_hub_status(&response.payload)?;
            if status.wifi_state == link_state::CONNECTED
                && status.mqtt_state == link_state::CONNECTED
            {
                break;
            }
            println!(
                "  waiting: wifi {} mqtt {} ip {:?}",
                status.wifi_state, status.mqtt_state, status.ip
            );
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "hub did not reach wifi+mqtt connected within 90 s"
        );
        std::thread::sleep(Duration::from_secs(2));
    }

    // The topic id is the hex tail of the USB serial (WTH-XXXXXX).
    let port = serialport::available_ports()?;
    for info in port {
        let serialport::SerialPortType::UsbPort(usb) = &info.port_type else {
            continue;
        };
        if let Some(serial) = usb.serial_number.as_deref() {
            if let Some(id) = serial.strip_prefix("WTH-") {
                return Ok(id.to_string());
            }
        }
    }
    anyhow::bail!("could not read the hub id from the USB serial")
}

/// Drive the rumqttc connection until a message on `topic` arrives.
fn wait_for_message(
    connection: &mut rumqttc::Connection,
    topic: &str,
    timeout_secs: u64,
) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    for notification in connection.iter() {
        if let Ok(Event::Incoming(Packet::Publish(publish))) = notification {
            if publish.topic == topic {
                return Ok(String::from_utf8_lossy(&publish.payload).into_owned());
            }
        }
        if Instant::now() > deadline {
            break;
        }
    }
    anyhow::bail!("no message on {topic} within {timeout_secs} s")
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}
