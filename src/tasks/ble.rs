//! BLE task for command/response handling
//!
//! Implements the BLE host task that manages connections and routes
//! commands/responses through the Nordic UART Service.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use trouble_host::prelude::*;

use crate::ble::service::{NordicUartService, NUS_MAX_PACKET_SIZE};
use crate::commands::{CommandParser, ResponseSerialiser, ResponseStatus};
use crate::config;
use crate::dispatcher::{CommandEnvelope, CommandSource, ResponseMessage, COMMAND_CHANNEL, RESPONSE_CHANNEL};
use crate::protocol::framing::FrameAccumulator;

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

/// Main BLE task that manages the Bluetooth stack and connections
///
/// This task:
/// 1. Initialises the BLE controller
/// 2. Starts advertising as "WalkieTextie-XXXXXX" (unique per device)
/// 3. Handles connections and GATT events
/// 4. Routes received data to COMMAND_CHANNEL
/// 5. Sends responses via notifications
pub async fn ble_task<C: Controller>(controller: C, device_id: [u8; 3]) {
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
        let parser = CommandParser::new();
        let serialiser = ResponseSerialiser::new();
        let command_sender = COMMAND_CHANNEL.sender();

        loop {
            // Start advertising
            crate::debug!("BLE: Advertising...");
            let advertiser = match peripheral
                .advertise(
                    &Default::default(),
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

            // Handle this connection
            let mut accumulator = FrameAccumulator::new();
            let mut sequence_id: u16 = 0;

            // Subscribe to unified response channel for this connection
            // Subscriber is dropped when connection ends, so messages don't queue up
            let mut response_sub = match RESPONSE_CHANNEL.subscriber() {
                Ok(s) => s,
                Err(_) => continue,  // No subscriber slots available
            };

            loop {
                // Use select to handle GATT events and response messages
                let gatt_future = conn.next();
                let response_future = response_sub.next_message_pure();

                match embassy_futures::select::select(gatt_future, response_future).await {
                    embassy_futures::select::Either::First(gatt_event) => {
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
                                                    match decode_and_parse(&parser, frame) {
                                                        Ok(command) => {
                                                            let envelope = CommandEnvelope {
                                                                command,
                                                                source: CommandSource::Ble,
                                                                sequence_id,
                                                            };
                                                            let _ = command_sender.try_send(envelope);
                                                        }
                                                        Err(response) => {
                                                            // Send error response directly via notification
                                                            let encoded = serialiser.serialise(&response);
                                                            let mut tx_buf = [0u8; NUS_MAX_PACKET_SIZE];
                                                            let len = encoded.len().min(tx_buf.len());
                                                            tx_buf[..len].copy_from_slice(&encoded[..len]);
                                                            let _ = server.nus.tx.notify(&conn, &tx_buf).await;
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
                    embassy_futures::select::Either::Second(msg) => {
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
                            let encoded = serialiser.serialise(&response);
                            let mut tx_buf = [0u8; NUS_MAX_PACKET_SIZE];
                            let len = encoded.len().min(tx_buf.len());
                            tx_buf[..len].copy_from_slice(&encoded[..len]);
                            let _ = server.nus.tx.notify(&conn, &tx_buf).await;
                        }
                    }
                }
            }
            // response_sub dropped here - no longer receiving broadcasts
        }
    };

    embassy_futures::select::select(runner_task, peripheral_task).await;
}

/// Decode COBS frame and parse command
fn decode_and_parse(
    parser: &CommandParser,
    mut frame: heapless::Vec<u8, { config::protocol::MAX_FRAME_SIZE }>,
) -> Result<crate::commands::Command, crate::commands::Response> {
    use crate::commands::serialiser::cobs_decode;

    // Add back the zero delimiter that FrameAccumulator strips
    let _ = frame.push(0x00);

    // Decode COBS
    let decoded = match cobs_decode(&frame) {
        Ok(d) => d,
        Err(_) => return Err(crate::commands::Response::error_raw(
            ResponseStatus::CrcError,  // Use CrcError for decode failures
            0x00,
        )),
    };

    if decoded.is_empty() {
        return Err(crate::commands::Response::error_raw(
            ResponseStatus::InvalidLength,
            0x00,
        ));
    }

    let command_id = decoded[0];

    parser.parse(&decoded).map_err(|status| {
        crate::commands::Response::error_raw(status, command_id)
    })
}
