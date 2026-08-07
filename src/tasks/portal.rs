//! Provisioning portal socket tasks (DHCP, DNS, HTTP) on the SoftAP stack.
//!
//! All three park on `wait_link_up()` until the WiFi task actually starts
//! the access point, so they cost nothing in normal station operation.

use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpEndpoint, Stack};
use embassy_time::{Duration, Instant, Timer};
use hub_protocol::{AckStatus, HubCommand, HubResponse};

use crate::debug;
use crate::dispatcher::{PORTAL_REPLY, PORTAL_REQUEST};
use crate::hub_config::HubConfig;
use crate::net::stats::HUB_STATS;
use crate::portal::http::{
    commands_from_update, config_json, route, status_json, try_parse, HttpRequest, ParseOutcome,
    Route, UpdateError, INDEX_HTML, MAX_REQUEST,
};
use crate::portal::{dhcp, dns};
use crate::tasks::admin::{AdminCommand, ADMIN_CHANNEL};

/// One-lease DHCP server on the SoftAP network.
pub async fn portal_dhcp_task(stack: Stack<'static>) {
    stack.wait_link_up().await;
    debug!("Portal: DHCP server up");

    let mut rx_meta = [PacketMetadata::EMPTY; 2];
    let mut rx_buffer = [0u8; 640];
    let mut tx_meta = [PacketMetadata::EMPTY; 2];
    let mut tx_buffer = [0u8; 640];
    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    if socket.bind(67).is_err() {
        debug!("Portal: DHCP bind failed");
        return;
    }

    let broadcast: IpEndpoint = (core::net::Ipv4Addr::BROADCAST, 68).into();
    let mut datagram = [0u8; 600];
    let mut reply = [0u8; 300];
    loop {
        let len = socket
            .recv_from_with(|data, _| {
                let n = data.len().min(datagram.len());
                datagram[..n].copy_from_slice(&data[..n]);
                n
            })
            .await;
        let Some(request) = dhcp::parse_request(&datagram[..len]) else {
            continue;
        };
        let reply_len = dhcp::build_reply(&request, &mut reply);
        if socket.send_to(&reply[..reply_len], broadcast).await.is_err() {
            debug!("Portal: DHCP reply send failed");
        }
    }
}

/// Catch-all DNS so every hostname lands on the portal.
pub async fn portal_dns_task(stack: Stack<'static>) {
    stack.wait_link_up().await;

    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buffer = [0u8; 1024];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_buffer = [0u8; 1024];
    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    if socket.bind(53).is_err() {
        debug!("Portal: DNS bind failed");
        return;
    }

    let mut query = [0u8; 320];
    let mut reply = [0u8; 512];
    loop {
        let (len, remote) = socket
            .recv_from_with(|data, meta| {
                let n = data.len().min(query.len());
                query[..n].copy_from_slice(&data[..n]);
                (n, meta.endpoint)
            })
            .await;
        if let Some(reply_len) = dns::answer(&query[..len], &mut reply) {
            let _ = socket.send_to(&reply[..reply_len], remote).await;
        }
    }
}

/// The config page and JSON API.
pub async fn portal_http_task(stack: Stack<'static>, config: &'static HubConfig) {
    stack.wait_link_up().await;
    debug!("Portal: HTTP server up at 192.168.4.1");

    let mut rx_buffer = [0u8; MAX_REQUEST];
    let mut tx_buffer = [0u8; 2048];
    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));
        if socket.accept(80).await.is_err() {
            continue;
        }
        let reboot = serve_connection(&mut socket, config).await;
        let _ = socket.flush().await;
        socket.close();
        Timer::after(Duration::from_millis(50)).await;
        socket.abort();
        drop(socket);
        if reboot {
            // Let the response reach the phone before restarting.
            Timer::after(Duration::from_millis(500)).await;
            debug!("Portal: configuration saved, rebooting");
            let _ = ADMIN_CHANNEL.try_send(AdminCommand::Reboot);
            core::future::pending::<()>().await;
        }
    }
}

/// Handle one connection (one request). Returns true when the hub should
/// reboot to apply saved settings.
async fn serve_connection(socket: &mut TcpSocket<'_>, config: &'static HubConfig) -> bool {
    let mut buf = [0u8; MAX_REQUEST];
    let mut have = 0usize;

    loop {
        match socket.read(&mut buf[have..]).await {
            Ok(0) | Err(_) => return false,
            Ok(n) => have += n,
        }
        // Borrow the parse result inside a scope so reading can continue.
        let outcome_action = match try_parse(&buf[..have]) {
            ParseOutcome::Incomplete => {
                if have >= MAX_REQUEST {
                    return false;
                }
                continue;
            }
            ParseOutcome::Bad => return false,
            ParseOutcome::Complete(request) => respond(socket, &request, config).await,
        };
        return outcome_action.unwrap_or(false);
    }
}

/// Send the response for one parsed request; None on socket error.
async fn respond(
    socket: &mut TcpSocket<'_>,
    request: &HttpRequest<'_>,
    config: &'static HubConfig,
) -> Option<bool> {
    match route(request) {
        Route::Index => {
            write_response(socket, "200 OK", "text/html", INDEX_HTML.as_bytes()).await?;
            Some(false)
        }
        Route::ApiConfigGet => {
            let body = config_json(config);
            write_response(socket, "200 OK", "application/json", &body).await?;
            Some(false)
        }
        Route::ApiStatus => {
            let uptime = Instant::now().as_secs() as u32;
            let body = status_json(&HUB_STATS.snapshot(uptime));
            write_response(socket, "200 OK", "application/json", &body).await?;
            Some(false)
        }
        Route::ApiConfigPost(body) => {
            let commands = match commands_from_update(body, config) {
                Ok(commands) => commands,
                Err(UpdateError::Bad) => {
                    write_response(
                        socket,
                        "400 Bad Request",
                        "application/json",
                        br#"{"ok":false,"error":"invalid values"}"#,
                    )
                    .await?;
                    return Some(false);
                }
                Err(UpdateError::Empty) => {
                    write_response(
                        socket,
                        "400 Bad Request",
                        "application/json",
                        br#"{"ok":false,"error":"nothing to change"}"#,
                    )
                    .await?;
                    return Some(false);
                }
            };
            for command in commands {
                if !apply_via_hub_ctrl(command).await {
                    write_response(
                        socket,
                        "422 Unprocessable Entity",
                        "application/json",
                        br#"{"ok":false,"error":"rejected or not stored"}"#,
                    )
                    .await?;
                    return Some(false);
                }
            }
            write_response(
                socket,
                "200 OK",
                "application/json",
                br#"{"ok":true,"rebooting":true}"#,
            )
            .await?;
            Some(true)
        }
        Route::Redirect => {
            let head = "HTTP/1.1 302 Found\r\nLocation: http://192.168.4.1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            write_all_tcp(socket, head.as_bytes()).await?;
            Some(false)
        }
    }
}

/// write_all over the socket's inherent write (the firmware's embedded-io
/// trait version predates the one embassy-net implements).
async fn write_all_tcp(socket: &mut TcpSocket<'_>, mut data: &[u8]) -> Option<()> {
    while !data.is_empty() {
        match socket.write(data).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => data = &data[n..],
        }
    }
    Some(())
}

/// Round-trip one command through the hub control task.
async fn apply_via_hub_ctrl(command: HubCommand) -> bool {
    PORTAL_REQUEST.send(command).await;
    matches!(
        PORTAL_REPLY.receive().await,
        HubResponse::ConfigAck { status: AckStatus::Ok }
    )
}

async fn write_response(
    socket: &mut TcpSocket<'_>,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Option<()> {
    let mut head: heapless::String<128> = heapless::String::new();
    let _ = core::fmt::Write::write_fmt(
        &mut head,
        format_args!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        ),
    );
    write_all_tcp(socket, head.as_bytes()).await?;
    write_all_tcp(socket, body).await
}
