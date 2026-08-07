//! Full end-to-end message tests: BLE node <-> LoRa <-> hub <-> MQTT broker.
//!
//! Needs: a provisioned hub online (bridge mode, connected to the broker),
//! a Walkie-Textie node advertising over BLE, and the broker reachable from
//! this machine. Sends uplinks from the node over BLE and asserts they land
//! on the broker as JSON; publishes downlinks and asserts the node hears
//! them over the air, with the tx result correlated.
//!
//! Run: cargo ble-e2e -- --broker-host 127.0.0.1

mod ble_client;
mod device;
mod protocol;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};

use ble_client::BleClient;
use protocol::ResponseId;

#[derive(Parser)]
struct Args {
    /// Broker address as this machine reaches it
    #[arg(long, default_value = "127.0.0.1")]
    broker_host: String,
    #[arg(long, default_value_t = 1883)]
    broker_port: u16,
    /// BLE name (prefix) of the node device
    #[arg(long, default_value = "WalkieTextie")]
    ble_name: String,
    /// How many message pairs to exchange in the soak loop
    #[arg(long, default_value_t = 3)]
    rounds: u32,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    match run(&args).await {
        Ok(()) => println!("{}", "All BLE end-to-end tests passed".green().bold()),
        Err(err) => {
            eprintln!("{} {err:#}", "FAILED:".red().bold());
            std::process::exit(1);
        }
    }
}

async fn run(args: &Args) -> Result<()> {
    println!("{}", "BLE node <-> hub <-> MQTT end-to-end test".bold());

    // Broker side first: find the hub via its retained status.
    let mut options = MqttOptions::new(
        format!("ble-e2e-{}", std::process::id()),
        &args.broker_host,
        args.broker_port,
    );
    options.set_keep_alive(Duration::from_secs(10));
    let (client, mut eventloop) = AsyncClient::new(options, 64);
    client.subscribe("wt/hub/+/status", QoS::AtLeastOnce).await?;

    let (hub_id, status_json) = wait_for_topic_suffix(&mut eventloop, "/status", 15)
        .await
        .context("no retained hub status on the broker - is the hub online?")?;
    anyhow::ensure!(
        status_json.contains("\"online\":true"),
        "hub status is not online: {status_json}"
    );
    println!("  hub {hub_id} online: {status_json}");

    let rx_topic = format!("wt/hub/{hub_id}/rx");
    let tx_topic = format!("wt/hub/{hub_id}/tx");
    let result_topic = format!("wt/hub/{hub_id}/tx/result");
    client.subscribe(&rx_topic, QoS::AtLeastOnce).await?;
    client.subscribe(&result_topic, QoS::AtLeastOnce).await?;
    // subscribe() only queues; pump the event loop until both SUBACKs land,
    // otherwise a fast uplink can beat the subscription to the broker.
    await_subacks(&mut eventloop, 2, 10).await?;

    // BLE side: connect to the node.
    println!("  scanning for BLE node {}...", args.ble_name);
    let node = BleClient::connect_by_name(&args.ble_name, Duration::from_secs(20))
        .await
        .context("could not connect to the node over BLE")?;
    println!("  {} connected to the node over BLE", "ok".green());

    // Pin the node to the hub's radio settings: SF is runtime-adjustable and
    // survives across sessions, so a stale value silently breaks the link.
    let config = node
        .send_command(protocol::CommandId::GetRadioConfig, &[], Duration::from_secs(10))
        .await?;
    println!("  node radio config payload: {:02x?}", config.payload);
    node.set_spreading_factor(11, Duration::from_secs(10)).await?;
    println!("  {} node pinned to SF11", "ok".green());

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() % 100_000)
        .unwrap_or(0);

    // Uplink: node -> LoRa -> hub -> broker.
    let uplink = format!("e2e-up-{nonce}");
    let response = node
        .lora_tx(uplink.as_bytes(), Duration::from_secs(15))
        .await?;
    anyhow::ensure!(
        response.resp_id == ResponseId::TxComplete,
        "node TX failed: {:?}",
        response.resp_id
    );
    let rx_json = wait_for_payload_containing(
        &mut eventloop,
        &rx_topic,
        &hex(uplink.as_bytes()),
        30,
    )
    .await
    .context("uplink never reached the broker")?;
    anyhow::ensure!(rx_json.contains("\"rssi\":"), "rx JSON missing rssi: {rx_json}");
    println!("  {} uplink: BLE -> LoRa -> MQTT ({rx_json})", "ok".green());

    // Downlink: broker -> hub -> LoRa -> node (over BLE notification).
    let downlink = format!("e2e-down-{nonce}");
    node.clear_buffer().await;
    client
        .publish(
            &tx_topic,
            QoS::AtLeastOnce,
            false,
            format!(r#"{{"payload_hex":"{}","id":{nonce}}}"#, hex(downlink.as_bytes())),
        )
        .await?;
    // publish() only queues; pump until the broker acknowledges it, because
    // the BLE wait below does not poll the MQTT event loop.
    await_puback(&mut eventloop, 10).await?;
    node.wait_for_rx_packet_matching(downlink.as_bytes(), Duration::from_secs(30))
        .await
        .context("node never heard the downlink over the air")?;
    let result_json = wait_for_payload_containing(
        &mut eventloop,
        &result_topic,
        "\"result\":\"sent\"",
        15,
    )
    .await
    .context("no tx result on the broker")?;
    anyhow::ensure!(
        result_json.contains(&format!("\"id\":{nonce}")),
        "tx result lost its correlation id: {result_json}"
    );
    println!("  {} downlink: MQTT -> LoRa -> BLE with correlated result", "ok".green());

    // Soak: alternate directions round after round. LoRa is a lossy medium
    // and both radios are half-duplex, so each direction gets one retry; the
    // hub's tx/result is asserted so a duty-cycle refusal cannot masquerade
    // as air loss.
    for round in 0..args.rounds {
        let up = format!("e2e-soak-up-{nonce}-{round}");
        let mut delivered = false;
        for attempt in 0..2 {
            let response = node.lora_tx(up.as_bytes(), Duration::from_secs(15)).await?;
            anyhow::ensure!(
                response.resp_id == ResponseId::TxComplete,
                "soak round {round}: node TX failed"
            );
            if wait_for_payload_containing(&mut eventloop, &rx_topic, &hex(up.as_bytes()), 20)
                .await
                .is_ok()
            {
                delivered = true;
                if attempt > 0 {
                    println!("  note: soak round {round} uplink needed a retry");
                }
                break;
            }
        }
        anyhow::ensure!(delivered, "soak round {round}: uplink lost twice");

        tokio::time::sleep(Duration::from_millis(500)).await;

        let down = format!("e2e-soak-down-{nonce}-{round}");
        let mut delivered = false;
        for attempt in 0..2 {
            node.clear_buffer().await;
            client
                .publish(
                    &tx_topic,
                    QoS::AtLeastOnce,
                    false,
                    format!(r#"{{"payload_hex":"{}"}}"#, hex(down.as_bytes())),
                )
                .await?;
            await_puback(&mut eventloop, 10).await?;
            // The hub must report the transmission happened.
            let result =
                wait_for_payload_containing(&mut eventloop, &result_topic, "\"result\":", 15)
                    .await
                    .with_context(|| format!("soak round {round}: no tx result"))?;
            anyhow::ensure!(
                result.contains("\"result\":\"sent\""),
                "soak round {round}: hub refused the downlink: {result}"
            );
            if node
                .wait_for_rx_packet_matching(down.as_bytes(), Duration::from_secs(20))
                .await
                .is_ok()
            {
                delivered = true;
                if attempt > 0 {
                    println!("  note: soak round {round} downlink needed a retry");
                }
                break;
            }
        }
        anyhow::ensure!(delivered, "soak round {round}: downlink lost twice");
        println!("  {} soak round {} both directions", "ok".green(), round + 1);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Bad downlinks must produce error results, not silence.
    client
        .publish(&tx_topic, QoS::AtLeastOnce, false, "not json at all")
        .await?;
    let error_json =
        wait_for_payload_containing(&mut eventloop, &result_topic, "bad_json", 15)
            .await
            .context("malformed downlink produced no error result")?;
    println!("  {} malformed downlink rejected: {error_json}", "ok".green());

    node.disconnect().await.ok();
    Ok(())
}

/// Pump the event loop until the queued QoS1 publish is acknowledged.
async fn await_puback(eventloop: &mut rumqttc::EventLoop, timeout_secs: u64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("timed out waiting for PUBACK")?;
        let event = tokio::time::timeout(remaining, eventloop.poll())
            .await
            .context("timed out waiting for PUBACK")??;
        if let Event::Incoming(Packet::PubAck(_)) = event {
            return Ok(());
        }
    }
}

/// Pump the event loop until `count` SUBACKs have arrived.
async fn await_subacks(
    eventloop: &mut rumqttc::EventLoop,
    count: usize,
    timeout_secs: u64,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut seen = 0;
    while seen < count {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("timed out waiting for SUBACK")?;
        let event = tokio::time::timeout(remaining, eventloop.poll())
            .await
            .context("timed out waiting for SUBACK")??;
        if let Event::Incoming(Packet::SubAck(_)) = event {
            seen += 1;
        }
    }
    Ok(())
}

/// Pump the event loop until a publish on a topic with the given suffix
/// arrives; returns (hub id from the topic, payload).
async fn wait_for_topic_suffix(
    eventloop: &mut rumqttc::EventLoop,
    suffix: &str,
    timeout_secs: u64,
) -> Result<(String, String)> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("timed out")?;
        let event = tokio::time::timeout(remaining, eventloop.poll())
            .await
            .context("timed out")??;
        if let Event::Incoming(Packet::Publish(publish)) = event {
            if let Some(rest) = publish.topic.strip_suffix(suffix) {
                let hub_id = rest.rsplit('/').next().unwrap_or_default().to_string();
                return Ok((hub_id, String::from_utf8_lossy(&publish.payload).into_owned()));
            }
        }
    }
}

/// Pump the event loop until a publish on `topic` whose payload contains
/// `needle` arrives.
async fn wait_for_payload_containing(
    eventloop: &mut rumqttc::EventLoop,
    topic: &str,
    needle: &str,
    timeout_secs: u64,
) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .with_context(|| format!("timed out waiting for {needle} on {topic}"))?;
        let event = tokio::time::timeout(remaining, eventloop.poll())
            .await
            .with_context(|| format!("timed out waiting for {needle} on {topic}"))??;
        if let Event::Incoming(Packet::Publish(publish)) = event {
            if publish.topic != topic {
                continue;
            }
            let payload = String::from_utf8_lossy(&publish.payload).into_owned();
            if payload.contains(needle) {
                return Ok(payload);
            }
        }
    }
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}
