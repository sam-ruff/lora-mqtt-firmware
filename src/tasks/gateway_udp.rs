//! Gateway UDP task: the Semtech UDP forwarder socket.
//!
//! Thin shell around [`GatewayForwarder`]. Pumps three things: datagrams
//! from the network server (acks and PULL_RESP downlinks), uplinks from the
//! radio task, and a one-second tick that drives the PULL_DATA keepalive
//! (every 10 s, holds the NAT pinhole open for downlinks) and the stat
//! report (every 30 s).

use embassy_futures::select::{select3, Either3};
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpEndpoint, Stack};
use embassy_time::{Duration, Ticker, Timer};

use crate::debug;
use crate::gateway::forwarder::{DatagramOutcome, GatewayForwarder};
use crate::gateway::udp_protocol::{TxAckError, ACK_BUF, PUSH_BUF};
use crate::gateway::GW_DOWNLINK_CHANNEL;
use crate::hub_config::HubConfig;
use crate::net::backoff::Backoff;
use crate::net::stats::HUB_STATS;
use crate::tasks::mqtt::resolve_host;

const PULL_DATA_INTERVAL_SECS: u32 = 10;
const STAT_INTERVAL_SECS: u32 = 30;
/// Local UDP port; also the conventional Semtech forwarder port.
const LOCAL_PORT: u16 = 1700;

pub async fn gateway_udp_task(stack: Stack<'static>, config: &'static HubConfig, mac: [u8; 6]) {
    if config.gateway.host.is_empty() {
        debug!("Gateway: no network server configured; provision over USB serial");
        core::future::pending::<()>().await;
    }

    let mut forwarder = GatewayForwarder::new(&mac);
    let uplink_receiver = crate::gateway::GW_UPLINK_CHANNEL.receiver();

    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buffer = [0u8; 2048];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_buffer = [0u8; 2048];

    stack.wait_config_up().await;

    // Resolve the network server with backoff; DNS needs the stack up.
    let mut backoff = Backoff::new();
    let server: IpEndpoint = loop {
        match resolve_host(stack, config.gateway.host.as_str()).await {
            Some(address) => break (address, config.gateway.port).into(),
            None => {
                debug!("Gateway: cannot resolve {}", config.gateway.host.as_str());
                Timer::after(Duration::from_secs(backoff.next_secs() as u64)).await;
            }
        }
    };

    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    if let Err(err) = socket.bind(LOCAL_PORT) {
        debug!("Gateway: UDP bind failed: {:?}", err);
        return;
    }
    debug!("Gateway: forwarding to {}:{}", config.gateway.host.as_str(), config.gateway.port);

    let mut datagram = [0u8; 1024];
    let mut out = [0u8; PUSH_BUF];
    let mut ack = [0u8; ACK_BUF];
    let mut ticker = Ticker::every(Duration::from_secs(1));
    let mut pull_elapsed = PULL_DATA_INTERVAL_SECS; // fire immediately at start
    let mut stat_elapsed = 0u32;

    loop {
        match select3(
            socket.recv_from_with(|data, _meta| {
                let len = data.len().min(datagram.len());
                datagram[..len].copy_from_slice(&data[..len]);
                len
            }),
            uplink_receiver.receive(),
            ticker.next(),
        )
        .await
        {
            Either3::First(len) => {
                let now = embassy_time::Instant::now();
                let outcome = forwarder.on_datagram(
                    now.as_micros() as u32,
                    now.as_millis(),
                    &datagram[..len],
                    &mut ack,
                );
                match outcome {
                    DatagramOutcome::None => {}
                    DatagramOutcome::Reply(ack_len) => {
                        send(&socket, &ack[..ack_len], server).await;
                    }
                    DatagramOutcome::Accepted { job, token } => {
                        let error = if GW_DOWNLINK_CHANNEL.try_send(job).is_ok() {
                            TxAckError::None
                        } else {
                            HUB_STATS.count_dropped(1);
                            TxAckError::CollisionPacket
                        };
                        let ack_len = forwarder.tx_ack(&mut ack, token, error);
                        send(&socket, &ack[..ack_len], server).await;
                    }
                }
            }
            Either3::Second((meta, payload)) => {
                let len = forwarder.on_uplink(&mut out, &meta, &payload);
                send(&socket, &out[..len], server).await;
            }
            Either3::Third(_) => {
                pull_elapsed += 1;
                stat_elapsed += 1;
                if pull_elapsed >= PULL_DATA_INTERVAL_SECS {
                    pull_elapsed = 0;
                    let len = forwarder.on_keepalive(&mut out);
                    send(&socket, &out[..len], server).await;
                }
                if stat_elapsed >= STAT_INTERVAL_SECS {
                    stat_elapsed = 0;
                    let len = forwarder.on_stat(&mut out);
                    send(&socket, &out[..len], server).await;
                    let stats = forwarder.stats();
                    debug!(
                        "Gateway stat: rx {} fwd {} dl {} tx {}",
                        stats.rxnb, stats.rxfw, stats.dwnb, stats.txnb
                    );
                }
            }
        }
    }
}

async fn send(socket: &UdpSocket<'_>, data: &[u8], server: IpEndpoint) {
    if let Err(err) = socket.send_to(data, server).await {
        debug!("Gateway: UDP send failed: {:?}", err);
    }
}
