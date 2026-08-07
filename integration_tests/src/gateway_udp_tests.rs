//! Gateway-mode Semtech UDP forwarder test.
//!
//! Needs one hub board (USB serial LMH-...) and the WiFi credentials already
//! provisioned (run `cargo hub` first, or pass --wifi-ssid/--wifi-password).
//! This host binds the network-server side (UDP 1700) and verifies the
//! forwarder protocol against the live gateway: PULL_DATA keepalives with the
//! right EUI, stat reports, and PULL_RESP downlink handling (immediate
//! accepted with TX_ACK NONE, stale timed rejected TOO_LATE).
//!
//! The over-the-air uplink leg needs a real LoRaWAN device (or ChirpStack in
//! docker as the server instead of this binary); it is not asserted here.
//!
//! Run: cargo gateway -- --server-host <this machine's LAN IP>

mod device;
mod protocol;

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;

use device::{resolve_port_with_prefix, DeviceClient};
use protocol::{gateway_config_payload, wifi_config_payload, HubCommandId};

const PROTOCOL_VERSION: u8 = 2;
const PUSH_DATA: u8 = 0x00;
const PUSH_ACK: u8 = 0x01;
const PULL_DATA: u8 = 0x02;
const PULL_RESP: u8 = 0x03;
const PULL_ACK: u8 = 0x04;
const TX_ACK: u8 = 0x05;

#[derive(Parser)]
struct Args {
    /// Hub board data port ("auto" detects by the LMH- USB serial)
    #[arg(long, default_value = "auto")]
    hub_port: String,
    /// This machine's address as the hub reaches it
    #[arg(long)]
    server_host: String,
    #[arg(long, default_value_t = 1700)]
    server_port: u16,
    /// Also (re)provision WiFi before switching modes
    #[arg(long, env = "HUB_WIFI_SSID", default_value = "")]
    wifi_ssid: String,
    #[arg(long, env = "HUB_WIFI_PASSWORD", default_value = "")]
    wifi_password: String,
    /// Skip provisioning and reboot (hub already in gateway mode)
    #[arg(long)]
    skip_provision: bool,
}

fn main() {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => println!("{}", "All gateway tests passed".green().bold()),
        Err(err) => {
            eprintln!("{} {err:#}", "FAILED:".red().bold());
            std::process::exit(1);
        }
    }
}

fn run(args: &Args) -> Result<()> {
    println!("{}", "Gateway-mode Semtech UDP forwarder test".bold());

    let socket = UdpSocket::bind(("0.0.0.0", args.server_port))
        .context("bind UDP 1700 (is another forwarder bridge running?)")?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;

    if !args.skip_provision {
        let hub_port = resolve_port_with_prefix(&args.hub_port, "LMH-")?;
        println!("  hub: {hub_port}");
        let mut hub = DeviceClient::new(&hub_port, 115200)?;
        hub.wait_ready(Duration::from_secs(5))?;

        if !args.wifi_ssid.is_empty() {
            hub.expect_config_ack(
                HubCommandId::SetWifiConfig,
                &wifi_config_payload(&args.wifi_ssid, &args.wifi_password),
            )
            .context("SetWifiConfig")?;
        }
        hub.expect_config_ack(
            HubCommandId::SetGatewayConfig,
            &gateway_config_payload(
                868_100_000,
                7,
                125,
                5,
                &args.server_host,
                args.server_port,
            ),
        )
        .context("SetGatewayConfig")?;
        // LoRaWAN gateway mode.
        hub.expect_config_ack(HubCommandId::SetMode, &[1]).context("SetMode")?;
        println!("  provisioned; rebooting hub into gateway mode");
        hub.reboot()?;
        drop(hub);
    }

    // The gateway sends PULL_DATA within ~10 s of the network coming up.
    println!("  waiting for the gateway to appear (up to 120 s)...");
    let (eui, gateway_addr) = wait_pull_data(&socket, Duration::from_secs(120))?;
    println!(
        "  {} PULL_DATA from {} (EUI {})",
        "ok".green(),
        gateway_addr,
        hex(&eui)
    );
    anyhow::ensure!(
        eui[3] == 0xFF && eui[4] == 0xFE,
        "gateway EUI must carry the FFFE insertion: {}",
        hex(&eui)
    );

    // Answer keepalives so the gateway sees the server as alive, and catch
    // the stat report (every 30 s, but the first one lands a full period
    // after the gateway's network comes up, which can trail the PULL_DATA
    // detection by most of a minute on a fresh boot).
    let stat = wait_stat(&socket, Duration::from_secs(90))?;
    println!("  {} stat report: {}", "ok".green(), stat);

    // Immediate downlink must be accepted with TX_ACK NONE (the hub's debug
    // port additionally logs "Gateway TX: 2 bytes sent").
    let token: u16 = 0x1234;
    let downlink = pull_resp(
        token,
        r#"{"txpk":{"imme":true,"freq":869.525,"powe":14,"modu":"LORA","datr":"SF9BW125","codr":"4/5","ipol":true,"data":"AQI="}}"#,
    );
    socket.send_to(&downlink, gateway_addr)?;
    let ack = wait_tx_ack(&socket, token, Duration::from_secs(10))?;
    anyhow::ensure!(
        ack.contains("NONE"),
        "immediate downlink should be accepted, got {ack}"
    );
    println!("  {} immediate downlink accepted (TX_ACK NONE)", "ok".green());

    // A timed downlink 100 ms in the past must be rejected TOO_LATE. tmst is
    // the gateway's internal counter; a tiny value is long since passed
    // (except during the first seconds after boot, which the keepalive wait
    // has already consumed).
    let token: u16 = 0x2345;
    let stale = pull_resp(
        token,
        r#"{"txpk":{"imme":false,"tmst":1000,"freq":868.1,"powe":14,"modu":"LORA","datr":"SF7BW125","codr":"4/5","ipol":true,"data":"AQI="}}"#,
    );
    socket.send_to(&stale, gateway_addr)?;
    let ack = wait_tx_ack(&socket, token, Duration::from_secs(10))?;
    anyhow::ensure!(
        ack.contains("TOO_LATE") || ack.contains("TOO_EARLY"),
        "stale downlink should be rejected, got {ack}"
    );
    println!("  {} stale downlink rejected ({})", "ok".green(), ack.trim());

    println!("  note: over-the-air uplink assertion needs a real LoRaWAN device");
    Ok(())
}

fn pull_resp(token: u16, json: &str) -> Vec<u8> {
    let mut data = vec![
        PROTOCOL_VERSION,
        (token >> 8) as u8,
        token as u8,
        PULL_RESP,
    ];
    data.extend_from_slice(json.as_bytes());
    data
}

/// Wait for a PULL_DATA, reply PULL_ACK, and return (EUI, gateway address).
fn wait_pull_data(
    socket: &UdpSocket,
    timeout: Duration,
) -> Result<([u8; 8], std::net::SocketAddr)> {
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 1024];
    while Instant::now() < deadline {
        let Ok((len, addr)) = socket.recv_from(&mut buf) else {
            continue;
        };
        if len >= 12 && buf[0] == PROTOCOL_VERSION && buf[3] == PULL_DATA {
            let ack = [PROTOCOL_VERSION, buf[1], buf[2], PULL_ACK];
            socket.send_to(&ack, addr)?;
            let mut eui = [0u8; 8];
            eui.copy_from_slice(&buf[4..12]);
            return Ok((eui, addr));
        }
        // Ack PUSH_DATA (stat messages) so the ack ratio stays healthy.
        if len >= 12 && buf[0] == PROTOCOL_VERSION && buf[3] == PUSH_DATA {
            let ack = [PROTOCOL_VERSION, buf[1], buf[2], PUSH_ACK];
            socket.send_to(&ack, addr)?;
        }
    }
    anyhow::bail!("no PULL_DATA within {timeout:?} - is the hub on the WiFi and in gateway mode?")
}

/// Wait for a PUSH_DATA carrying a stat object; ack it and return the JSON.
fn wait_stat(socket: &UdpSocket, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 1024];
    while Instant::now() < deadline {
        let Ok((len, addr)) = socket.recv_from(&mut buf) else {
            continue;
        };
        if len < 12 || buf[0] != PROTOCOL_VERSION {
            continue;
        }
        match buf[3] {
            PUSH_DATA => {
                let ack = [PROTOCOL_VERSION, buf[1], buf[2], PUSH_ACK];
                socket.send_to(&ack, addr)?;
                let json = String::from_utf8_lossy(&buf[12..len]);
                if json.contains("\"stat\"") {
                    return Ok(json.into_owned());
                }
            }
            PULL_DATA => {
                let ack = [PROTOCOL_VERSION, buf[1], buf[2], PULL_ACK];
                socket.send_to(&ack, addr)?;
            }
            _ => {}
        }
    }
    anyhow::bail!("no stat PUSH_DATA within {timeout:?}")
}

/// Wait for the TX_ACK echoing `token` and return its JSON payload.
fn wait_tx_ack(socket: &UdpSocket, token: u16, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 1024];
    while Instant::now() < deadline {
        let Ok((len, addr)) = socket.recv_from(&mut buf) else {
            continue;
        };
        if len < 12 || buf[0] != PROTOCOL_VERSION {
            continue;
        }
        match buf[3] {
            TX_ACK if u16::from_be_bytes([buf[1], buf[2]]) == token => {
                return Ok(String::from_utf8_lossy(&buf[12..len]).into_owned());
            }
            PULL_DATA => {
                let ack = [PROTOCOL_VERSION, buf[1], buf[2], PULL_ACK];
                socket.send_to(&ack, addr)?;
            }
            PUSH_DATA => {
                let ack = [PROTOCOL_VERSION, buf[1], buf[2], PUSH_ACK];
                socket.send_to(&ack, addr)?;
            }
            _ => {}
        }
    }
    anyhow::bail!("no TX_ACK for token {token:#06x} within {timeout:?}")
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02X}")).collect()
}
