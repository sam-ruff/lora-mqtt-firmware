//! BLE task for command/response handling
//!
//! Implements the BLE host task that manages connections and routes
//! commands/responses through the Nordic UART Service.

use bt_hci::cmd::le::{LeConnUpdate, LeReadLocalSupportedFeatures};
use bt_hci::controller::{ControllerCmdAsync, ControllerCmdSync};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::pubsub::WaitResult;
use embassy_time::{Duration, Ticker};
use trouble_host::prelude::*;

use crate::ble::service::{NordicUartService, NUS_MAX_PACKET_SIZE};
use crate::config;
use crate::dispatcher::{CommandEnvelope, CommandSource, ResponseMessage, COMMAND_CHANNEL, RESPONSE_CHANNEL};
use wt_protocol::{Command, FrameAccumulator, Response, ResponseStatus};

/// Device name prefix for BLE advertising
const DEVICE_NAME_PREFIX: &str = "WalkieTextie-";

/// Format device ID bytes as uppercase hex into a buffer
/// Returns the formatted string slice
fn format_device_name<'a>(buf: &'a mut [u8; 20], device_id: &[u8; 3]) -> &'a str {
    const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";
    let prefix = DEVICE_NAME_PREFIX.as_bytes();

    // Copy prefix
    buf[..prefix.len()].copy_from_slice(prefix);

    // Format 3 bytes as 6 hex characters
    let mut pos = prefix.len();
    for &byte in device_id {
        buf[pos] = HEX_CHARS[(byte >> 4) as usize];
        buf[pos + 1] = HEX_CHARS[(byte & 0x0F) as usize];
        pos += 2;
    }

    // All bytes are ASCII, so this will always succeed
    core::str::from_utf8(&buf[..pos]).unwrap_or(DEVICE_NAME_PREFIX)
}

/// Number of maximum concurrent connections
const CONNECTIONS_MAX: usize = 1;
/// Number of L2CAP channels
const L2CAP_CHANNELS_MAX: usize = 3;

/// BLE GATT Server with Nordic UART Service
#[gatt_server(mutex_type = CriticalSectionRawMutex)]
struct Server {
    nus: NordicUartService,
}

/// Notify a host-link frame to the central in NUS-sized chunks.
///
/// Each notification carries exactly the chunk bytes; the receiver
/// reassembles frames on the 0x00 delimiter.
async fn notify_frame<P: PacketPool>(
    tx: &Characteristic<heapless09::Vec<u8, NUS_MAX_PACKET_SIZE>>,
    conn: &GattConnection<'_, '_, P>,
    response: &Response,
) {
    let encoded = wt_protocol::encode_response(response);
    for chunk in encoded.chunks(NUS_MAX_PACKET_SIZE) {
        let mut buf: heapless09::Vec<u8, NUS_MAX_PACKET_SIZE> = heapless09::Vec::new();
        let _ = buf.extend_from_slice(chunk);
        let _ = tx.notify(conn, &buf).await;
    }
}

/// Main BLE task that manages the Bluetooth stack and connections
///
/// This task:
/// 1. Initialises the BLE controller
/// 2. Starts advertising as "WalkieTextie-XXXXXX" (unique per device)
/// 3. Handles connections and GATT events
/// 4. Routes received data to COMMAND_CHANNEL
/// 5. Sends responses via notifications
pub async fn ble_task<C>(controller: C, device_id: [u8; 3])
where
    C: Controller
        + ControllerCmdAsync<LeConnUpdate>
        + ControllerCmdSync<LeReadLocalSupportedFeatures>,
{
    // Generate unique device name from chip ID
    let mut device_name_buf = [0u8; 20];
    let device_name = format_device_name(&mut device_name_buf, &device_id);

    crate::debug!("BLE: Starting as '{}'", device_name);

    // Create BLE host resources
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();

    // Build the BLE stack with address derived from device ID
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random([
            device_id[0], device_id[1], device_id[2],
            0x1E, 0x83, 0xE7
        ]));

    let Host {
        mut peripheral,
        mut runner,
        ..
    } = stack.build();

    // Create GATT server with GAP configuration
    let gap = GapConfig::Peripheral(PeripheralConfig {
        name: device_name,
        appearance: &appearance::UNKNOWN,
    });
    let server: Server = match Server::new_with_config(gap) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Run both the BLE runner and peripheral logic concurrently using select
    let runner_task = runner.run();

    let peripheral_task = async {
        let mut adv_data = [0u8; 31];
        let len = match AdStructure::encode_slice(
            &[
                AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                AdStructure::CompleteLocalName(device_name.as_bytes()),
            ],
            &mut adv_data,
        ) {
            Ok(l) => l,
            Err(_) => return,
        };

        // Shared state for command processing
        let command_sender = COMMAND_CHANNEL.sender();

        // Advertise fast (default is 160 ms) so a central discovers the radio
        // quickly and a flaky GATT connect has more attempts to land.
        let adv_params = AdvertisementParameters {
            interval_min: Duration::from_millis(30),
            interval_max: Duration::from_millis(60),
            ..Default::default()
        };

        loop {
            // Start advertising
            crate::debug!("BLE: Advertising...");
            let advertiser = match peripheral
                .advertise(
                    &adv_params,
                    Advertisement::ConnectableScannableUndirected {
                        adv_data: &adv_data[..len],
                        scan_data: &[],
                    },
                )
                .await
            {
                Ok(a) => a,
                Err(_) => continue,
            };

            // Wait for connection
            let acceptor = match advertiser.accept().await {
                Ok(a) => {
                    crate::debug!("BLE: Connected");
                    a
                }
                Err(_) => continue,
            };

            // Attach to attribute server (using Deref to get &AttributeServer)
            let conn = match acceptor.with_attribute_server(&*server) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Request faster connection parameters with the default generous
            // supervision timeout. Web Bluetooth (Chrome on desktop and Android)
            // otherwise negotiates parameters that drop this link after ~6s;
            // native apps avoid it via high connection priority, which Web
            // Bluetooth cannot request, so we ask for it from the peripheral side.
            let conn_params = ConnectParams {
                min_connection_interval: Duration::from_millis(15),
                max_connection_interval: Duration::from_millis(30),
                max_latency: 0,
                ..Default::default()
            };
            if conn.raw().update_connection_params(&stack, &conn_params).await.is_err() {
                crate::debug!("BLE: connection parameter update failed");
            }

            // Handle this connection
            let mut accumulator = FrameAccumulator::new();
            let mut sequence_id: u16 = 0;

            // Subscribe to unified response channel for this connection
            // Subscriber is dropped when connection ends, so messages don't queue up
            let mut response_sub = match RESPONSE_CHANNEL.subscriber() {
                Ok(s) => s,
                Err(_) => continue,  // No subscriber slots available
            };

            // Keepalive: some BLE centrals (notably desktop Chrome + BlueZ) keep a
            // short supervision timeout and ignore our connection-parameter
            // request, dropping an otherwise-idle link after ~5s. Pushing a small
            // notification periodically guarantees the central keeps receiving
            // packets, so the link holds on every host. The host treats this as a
            // routine (unsolicited) Version response and ignores it.
            let mut keepalive = Ticker::every(Duration::from_millis(1500));

            loop {
                // Use select to handle GATT events, response messages and the
                // keepalive tick.
                let gatt_future = conn.next();
                let response_future = response_sub.next_message();
                let keepalive_future = keepalive.next();

                match embassy_futures::select::select3(gatt_future, response_future, keepalive_future).await {
                    embassy_futures::select::Either3::First(gatt_event) => {
                        match gatt_event {
                            GattConnectionEvent::Disconnected { reason: _ } => {
                                crate::debug!("BLE: Disconnected");
                                break;
                            }
                            GattConnectionEvent::Gatt { event } => {
                                match event {
                                    GattEvent::Write(write_event) => {
                                        // Check if this is a write to the RX characteristic
                                        if write_event.handle() == server.nus.rx.handle {
                                            let data = write_event.data();

                                            // Process each byte through the accumulator
                                            for &byte in data {
                                                if let Some(frame) = accumulator.push(byte) {
                                                    sequence_id = sequence_id.wrapping_add(1);

                                                    // Decode COBS and parse command
                                                    match decode_and_parse(frame) {
                                                        Ok(command) => {
                                                            let command_id = command.id();
                                                            let envelope = CommandEnvelope {
                                                                command,
                                                                source: CommandSource::Ble,
                                                                sequence_id,
                                                            };
                                                            if command_sender.try_send(envelope).is_err() {
                                                                // Queue full: tell the host rather than
                                                                // silently dropping the command and leaving
                                                                // it waiting on a response forever.
                                                                crate::debug!("BLE: command queue full, rejecting");
                                                                let response = Response::error(
                                                                    ResponseStatus::Timeout,
                                                                    command_id,
                                                                );
                                                                notify_frame(&server.nus.tx, &conn, &response).await;
                                                            }
                                                        }
                                                        Err(response) => {
                                                            // Send error response directly via notification
                                                            notify_frame(&server.nus.tx, &conn, &response).await;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        // Accept the write
                                        let _ = write_event.accept();
                                    }
                                    GattEvent::Read(read_event) => {
                                        let _ = read_event.accept();
                                    }
                                    GattEvent::Other(other_event) => {
                                        let _ = other_event.accept();
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    embassy_futures::select::Either3::Second(wait_result) => {
                        let msg = match wait_result {
                            // The BLE link fell behind the publisher and messages
                            // were dropped; make that visible instead of silent.
                            WaitResult::Lagged(count) => {
                                crate::debug!("BLE: response subscriber lagged, {} lost", count);
                                continue;
                            }
                            WaitResult::Message(msg) => msg,
                        };

                        // Filter and process response messages
                        let response = match msg {
                            ResponseMessage::Command { source, response, .. } => {
                                // Only process responses for BLE source
                                if source == CommandSource::Ble {
                                    Some(response)
                                } else {
                                    None
                                }
                            }
                            ResponseMessage::Unsolicited(response) => {
                                // Always process unsolicited packets
                                Some(response)
                            }
                        };

                        if let Some(response) = response {
                            // Chunk the frame across notifications: truncating
                            // it here used to break every response over 128
                            // bytes (any real message-sized RxPacket). The
                            // receiver reassembles on the 0x00 delimiter.
                            notify_frame(&server.nus.tx, &conn, &response).await;
                        }
                    }
                    embassy_futures::select::Either3::Third(_) => {
                        // Keepalive tick: send an unsolicited Version notification so
                        // the central keeps receiving packets and never hits its
                        // supervision timeout on an idle link.
                        let response = Response::Version {
                            major: config::protocol::VERSION_MAJOR,
                            minor: config::protocol::VERSION_MINOR,
                            patch: config::protocol::VERSION_PATCH,
                        };
                        notify_frame(&server.nus.tx, &conn, &response).await;
                    }
                }
            }
            // response_sub dropped here - no longer receiving broadcasts
        }
    };

    embassy_futures::select::select(runner_task, peripheral_task).await;
}

/// Decode a COBS frame (delimiter included) and parse it into a command.
fn decode_and_parse(
    frame: heapless::Vec<u8, { config::protocol::MAX_FRAME_SIZE }>,
) -> Result<Command, Response> {
    let decoded = match wt_protocol::cobs_decode(&frame) {
        Ok(d) => d,
        Err(_) => return Err(Response::error_raw(ResponseStatus::CrcError, 0x00)),
    };

    if decoded.is_empty() {
        return Err(Response::error_raw(ResponseStatus::InvalidLength, 0x00));
    }

    // Byte 1 is the command id (byte 0 is the protocol version); echoed back on error.
    let command_id = decoded.get(1).copied().unwrap_or(0);

    wt_protocol::parse_command(&decoded).map_err(|status| Response::error_raw(status, command_id))
}
