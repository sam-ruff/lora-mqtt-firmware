//! MQTT task: owns the TCP socket and rust-mqtt client lifecycle.
//!
//! Connects to the configured broker, announces a retained online status
//! (with an offline LWT), subscribes to the downlink topic and then pumps
//! three things in one select loop: incoming packets, queued outbound
//! publications and the keepalive ping. Any error tears the session down
//! and reconnects with backoff.

use core::net::Ipv4Addr;
use core::num::NonZeroU16;

use embassy_futures::select::{select3, Either3};
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, Stack};
use embassy_time::{Duration, Ticker, Timer};
use heapless::Vec;
use hub_protocol::LinkState;
use rust_mqtt::buffer::AllocBuffer;
use rust_mqtt::client::event::Event;
use rust_mqtt::client::options::{
    ConnectOptions, PublicationOptions, SubscriptionOptions, TopicReference, WillOptions,
};
use rust_mqtt::client::Client;
use rust_mqtt::config::KeepAlive;
use rust_mqtt::types::{MqttBinary, MqttString, TopicName};

use crate::bridge::codec::{status_json, MAX_JSON};
use crate::bridge::{MQTT_IN_CHANNEL, MQTT_OUT_CHANNEL};
use crate::config::protocol::{VERSION_MAJOR, VERSION_MINOR, VERSION_PATCH};
use crate::debug;
use crate::hub_config::HubConfig;
use crate::net::backoff::Backoff;
use crate::net::stats::HUB_STATS;
use crate::net::traits::{deliver, MqttSink, SinkError, TopicSet};

const KEEPALIVE_SECS: u16 = 30;
/// Ping at half the keepalive so one lost ping never expires the session.
const PING_INTERVAL: Duration = Duration::from_secs(KEEPALIVE_SECS as u64 / 2);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(90);

/// One MQTT connection's client type. Const generics: in-flight SUBSCRIBEs,
/// receive maximum, send maximum, subscription identifiers.
type MqttClient<'c> = Client<'c, TcpSocket<'c>, AllocBuffer, 2, 4, 4, 2>;

pub async fn mqtt_task(stack: Stack<'static>, config: &'static HubConfig, device_id: [u8; 3]) {
    if config.mqtt.host.is_empty() {
        HUB_STATS.set_mqtt_state(LinkState::Unprovisioned);
        debug!("MQTT: no broker configured; provision over USB serial");
        core::future::pending::<()>().await;
    }

    let topics = TopicSet::new(device_id);
    let client_id = resolve_client_id(config, device_id);
    let mut fw: heapless::String<16> = heapless::String::new();
    let _ = core::fmt::Write::write_fmt(
        &mut fw,
        format_args!("{}.{}.{}", VERSION_MAJOR, VERSION_MINOR, VERSION_PATCH),
    );

    let mut backoff = Backoff::new();
    let mut rx_buffer = [0u8; 2048];
    let mut tx_buffer = [0u8; 2048];

    loop {
        stack.wait_config_up().await;
        HUB_STATS.set_mqtt_state(LinkState::Connecting);

        let Some(address) = resolve_host(stack, config.mqtt.host.as_str()).await else {
            debug!("MQTT: cannot resolve {}", config.mqtt.host.as_str());
            HUB_STATS.set_mqtt_state(LinkState::Disconnected);
            Timer::after(Duration::from_secs(backoff.next_secs() as u64)).await;
            continue;
        };

        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(SOCKET_TIMEOUT));
        if let Err(err) = socket.connect((address, config.mqtt.port)).await {
            debug!("MQTT: TCP connect failed: {:?}", err);
            HUB_STATS.set_mqtt_state(LinkState::Disconnected);
            Timer::after(Duration::from_secs(backoff.next_secs() as u64)).await;
            continue;
        }

        let mut buffer = AllocBuffer;
        let mut client: MqttClient = Client::new(&mut buffer);
        match run_session(&mut client, socket, &topics, &client_id, &fw, &mut backoff).await {
            SessionEnd::Failed => {
                client.abort().await;
            }
        }
        HUB_STATS.set_mqtt_state(LinkState::Disconnected);
        debug!("MQTT: session ended, reconnecting");
        Timer::after(Duration::from_secs(backoff.next_secs() as u64)).await;
    }
}

enum SessionEnd {
    Failed,
}

async fn run_session<'c>(
    client: &mut MqttClient<'c>,
    socket: TcpSocket<'c>,
    topics: &TopicSet,
    client_id: &str,
    fw: &str,
    backoff: &mut Backoff,
) -> SessionEnd {
    let offline = status_json(false, None);
    let Some(status_topic) = topic_name(topics.status()) else {
        return SessionEnd::Failed;
    };
    let Ok(will_payload) = MqttBinary::try_from(offline.as_slice()) else {
        return SessionEnd::Failed;
    };
    let Some(keepalive) = NonZeroU16::new(KEEPALIVE_SECS) else {
        return SessionEnd::Failed;
    };
    let options = ConnectOptions::new()
        .clean_start()
        .keep_alive(KeepAlive::Seconds(keepalive))
        .will(WillOptions::new(status_topic, will_payload).retain());
    let Ok(identifier) = MqttString::try_from(client_id) else {
        return SessionEnd::Failed;
    };

    if let Err(err) = client.connect(socket, &options, Some(identifier)).await {
        debug!("MQTT: connect rejected: {:?}", err);
        return SessionEnd::Failed;
    }
    debug!("MQTT: connected as {}", client_id);
    HUB_STATS.set_mqtt_state(LinkState::Connected);
    backoff.reset();

    // Retained online status, then the downlink subscription.
    let online = status_json(true, Some(fw));
    let mut sink = ClientSink { client };
    if sink.publish(topics.status(), &online, true).await.is_err() {
        return SessionEnd::Failed;
    }
    let Some(tx_topic) = topic_name(topics.tx()) else {
        return SessionEnd::Failed;
    };
    if let Err(err) = sink
        .client
        .subscribe(tx_topic.into(), SubscriptionOptions::new().at_least_once())
        .await
    {
        debug!("MQTT: subscribe failed: {:?}", err);
        return SessionEnd::Failed;
    }

    let mut ping = Ticker::every(PING_INTERVAL);
    loop {
        // poll_header is cancel-safe; poll_body must then run to completion.
        match select3(
            sink.client.poll_header(),
            MQTT_OUT_CHANNEL.receive(),
            ping.next(),
        )
        .await
        {
            Either3::First(Ok(header)) => match sink.client.poll_body(header).await {
                Ok(Event::Publish(publish)) => {
                    let topic: &str = publish.topic.as_ref().as_ref();
                    if topic == topics.tx() {
                        let mut payload: Vec<u8, MAX_JSON> = Vec::new();
                        if payload.extend_from_slice(&publish.message).is_ok() {
                            if MQTT_IN_CHANNEL.try_send(payload).is_err() {
                                HUB_STATS.count_dropped(1);
                            }
                        } else {
                            HUB_STATS.count_dropped(1);
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    debug!("MQTT: poll failed: {:?}", err);
                    return SessionEnd::Failed;
                }
            },
            Either3::First(Err(err)) => {
                debug!("MQTT: link lost: {:?}", err);
                return SessionEnd::Failed;
            }
            Either3::Second(outbound) => {
                if deliver(&mut sink, topics, &outbound).await.is_err() {
                    return SessionEnd::Failed;
                }
            }
            Either3::Third(_) => {
                if sink.client.ping().await.is_err() {
                    return SessionEnd::Failed;
                }
            }
        }
    }
}

/// Production [`MqttSink`] over the rust-mqtt client.
struct ClientSink<'a, 'c> {
    client: &'a mut MqttClient<'c>,
}

impl MqttSink for ClientSink<'_, '_> {
    async fn publish(&mut self, topic: &str, payload: &[u8], retain: bool) -> Result<(), SinkError> {
        let name = topic_name(topic).ok_or(SinkError)?;
        let mut options = PublicationOptions::new(TopicReference::Name(name));
        options.retain = retain;
        self.client
            .publish(&options, payload.into())
            .await
            .map(|_| ())
            .map_err(|err| {
                debug!("MQTT: publish failed: {:?}", err);
                SinkError
            })
    }
}

fn topic_name(topic: &str) -> Option<TopicName<'_>> {
    TopicName::new(MqttString::try_from(topic).ok()?)
}

/// Literal IPv4 first, DNS A lookup otherwise.
async fn resolve_host(stack: Stack<'static>, host: &str) -> Option<IpAddress> {
    if let Ok(addr) = host.parse::<Ipv4Addr>() {
        return Some(IpAddress::Ipv4(addr));
    }
    let addresses = stack.dns_query(host, DnsQueryType::A).await.ok()?;
    addresses.first().copied()
}

fn resolve_client_id(config: &HubConfig, device_id: [u8; 3]) -> heapless::String<32> {
    if !config.mqtt.client_id.is_empty() {
        return config.mqtt.client_id.clone();
    }
    let mut id: heapless::String<32> = heapless::String::new();
    let _ = id.push_str("wt-hub-");
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in device_id {
        let _ = id.push(HEX[(byte >> 4) as usize] as char);
        let _ = id.push(HEX[(byte & 0x0F) as usize] as char);
    }
    id
}
