#![no_std]
#![no_main]

// Required for ESP-IDF bootloader compatibility
// Use explicit parameters to ensure correct efuse block revision values
esp_bootloader_esp_idf::esp_app_desc!(
    env!("CARGO_PKG_VERSION"),  // version
    env!("CARGO_PKG_NAME"),     // project_name
    "00:00:00",                 // build_time
    "2025-01-01",               // build_date
    "0.0.0",                    // idf_ver (not using IDF)
    0x10000,                    // mmu_page_size (64KB)
    0,                          // min_efuse_blk_rev_full (accept all)
    u16::MAX                    // max_efuse_blk_rev_full (accept all)
);

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Receiver, Sender};
use esp_backtrace as _;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_hal::Async;
use static_cell::StaticCell;

mod commands;
mod config;
mod dispatcher;
mod lora;
mod protocol;
mod serial;

use commands::{CommandParser, Response, ResponseSerialiser};
use dispatcher::{CommandDispatcher, CommandEnvelope, CommandSource, COMMAND_CHANNEL, SERIAL_RESPONSE_CHANNEL};
use lora::driver::{Sx1262Driver, Sx1262Pins};
use lora::traits::{LoraError, LoraRadio};
use protocol::framing::FrameAccumulator;
use serial::reader::ReadResult;

/// Polling interval for background LoRa receive (max TX latency)
const RX_POLL_INTERVAL_MS: u32 = 100;

/// Sequence ID for unsolicited responses (background RX packets)
const UNSOLICITED_SEQUENCE_ID: u16 = 0;

/// Type alias for the command channel sender
type CommandSender = Sender<'static, CriticalSectionRawMutex, CommandEnvelope, 8>;

/// Type alias for the command channel receiver
type CommandReceiver = Receiver<'static, CriticalSectionRawMutex, CommandEnvelope, 8>;

/// Type alias for the response channel
type ResponseReceiver = Receiver<'static, CriticalSectionRawMutex, (u16, Response), 4>;
type ResponseSender = Sender<'static, CriticalSectionRawMutex, (u16, Response), 4>;

/// LED flash duration configuration
#[derive(Clone, Copy)]
pub enum LedFlashDuration {
    /// Use the default flash duration
    Default,
    /// Use a custom flash duration in milliseconds
    Ms(u64),
}

/// Type alias for the LED flash channel
type LedSender = Sender<'static, CriticalSectionRawMutex, LedFlashDuration, 4>;
type LedReceiver = Receiver<'static, CriticalSectionRawMutex, LedFlashDuration, 4>;

/// Static executor for embassy
static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();

/// Channel for LED flash signals
static LED_CHANNEL: embassy_sync::channel::Channel<CriticalSectionRawMutex, LedFlashDuration, 4> =
    embassy_sync::channel::Channel::new();

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Turn on LED (active low)
    let led = Output::new(peripherals.GPIO48, Level::Low, OutputConfig::default());

    // Initialise the RTOS scheduler with timer - MUST be done before any async operations
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    // Configure SPI for LoRa
    let sclk = peripherals.GPIO7;
    let miso = peripherals.GPIO8;
    let mosi = peripherals.GPIO9;

    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(1))
            .with_mode(SpiMode::_0),
    )
    .unwrap()
    .with_sck(sclk)
    .with_miso(miso)
    .with_mosi(mosi)
    .into_async();


    // Configure LoRa control pins
    let nss = Output::new(peripherals.GPIO41, Level::High, OutputConfig::default());
    let dio1 = Input::new(peripherals.GPIO39, InputConfig::default().with_pull(Pull::Down));
    let nrst = Output::new(peripherals.GPIO42, Level::High, OutputConfig::default());
    let busy = Input::new(peripherals.GPIO40, InputConfig::default().with_pull(Pull::Down));

    let lora_pins = Sx1262Pins {
        nss,
        dio1,
        nrst,
        busy,
    };

    // Create LoRa driver
    let lora_driver = Sx1262Driver::new(spi, lora_pins);

    // Configure USB Serial JTAG for serial commands
    let usb_serial = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async();
    let (usb_rx, usb_tx) = usb_serial.split();

    // Create and run the embassy executor
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner| {
        spawner.must_spawn(async_main(spawner, usb_rx, usb_tx, lora_driver, led));
    })
}

#[embassy_executor::task]
async fn async_main(
    spawner: Spawner,
    usb_rx: esp_hal::usb_serial_jtag::UsbSerialJtagRx<'static, Async>,
    usb_tx: esp_hal::usb_serial_jtag::UsbSerialJtagTx<'static, Async>,
    lora_driver: Sx1262Driver<
        esp_hal::spi::master::Spi<'static, Async>,
        Output<'static>,
        Input<'static>,
        Output<'static>,
        Input<'static>,
    >,
    led: Output<'static>,
) {
    // Get channel handles
    let command_sender = COMMAND_CHANNEL.sender();
    let command_receiver = COMMAND_CHANNEL.receiver();
    let response_sender = SERIAL_RESPONSE_CHANNEL.sender();
    let response_receiver = SERIAL_RESPONSE_CHANNEL.receiver();
    let led_sender = LED_CHANNEL.sender();
    let led_receiver = LED_CHANNEL.receiver();

    // Spawn tasks
    spawner.spawn(serial_reader_task(usb_rx, command_sender, response_sender.clone())).unwrap();
    spawner.spawn(serial_writer_task(usb_tx, response_receiver)).unwrap();
    spawner.spawn(lora_task(lora_driver, command_receiver, response_sender, led_sender)).unwrap();
    spawner.spawn(led_task(led, led_receiver)).unwrap();
}

/// Task that reads commands from USB serial
#[embassy_executor::task]
async fn serial_reader_task(
    mut usb_rx: esp_hal::usb_serial_jtag::UsbSerialJtagRx<'static, Async>,
    command_sender: CommandSender,
    response_sender: ResponseSender,
) {
    let mut accumulator = FrameAccumulator::new();
    let parser = CommandParser::new();
    let mut sequence_counter: u16 = 0;

    loop {
        // Read bytes from USB serial
        let mut buf = [0u8; 64];
        match embedded_io_async::Read::read(&mut usb_rx, &mut buf).await {
            Ok(0) => continue,
            Ok(n) => {
                // Process each byte through the frame accumulator
                for &byte in &buf[..n] {
                    if let Some(frame) = accumulator.push(byte) {
                        // Frame complete, try to decode and parse
                        let seq_id = sequence_counter;
                        sequence_counter = sequence_counter.wrapping_add(1);

                        match process_frame(&parser, frame) {
                            Some(ReadResult::Command(cmd)) => {
                                let envelope = CommandEnvelope {
                                    command: cmd,
                                    source: CommandSource::Serial,
                                    sequence_id: seq_id,
                                };
                                command_sender.send(envelope).await;
                            }
                            Some(ReadResult::ParseError(status, cmd_id)) => {
                                let response = Response::error_raw(status, cmd_id);
                                response_sender.send((seq_id, response)).await;
                            }
                            None => {
                                // Invalid frame, ignore
                            }
                            Some(ReadResult::SerialError(_)) => {
                                // Shouldn't happen in frame processing
                            }
                        }
                    }
                }
            }
            Err(_) => {
                // UART error, just continue
                embassy_time::Timer::after(embassy_time::Duration::from_millis(10)).await;
            }
        }
    }
}

/// Process a complete COBS frame
fn process_frame(
    parser: &CommandParser,
    mut frame: heapless::Vec<u8, { config::protocol::MAX_FRAME_SIZE }>,
) -> Option<ReadResult> {
    use commands::serialiser::cobs_decode;

    // Add back the zero delimiter that FrameAccumulator strips
    // (corncobs::decode_buf expects it)
    let _ = frame.push(0x00);

    // Decode COBS
    let decoded = match cobs_decode(&frame) {
        Ok(d) => d,
        Err(_) => return None,
    };

    if decoded.is_empty() {
        return None;
    }

    let command_id = decoded[0];

    match parser.parse(&decoded) {
        Ok(cmd) => Some(ReadResult::Command(cmd)),
        Err(status) => Some(ReadResult::ParseError(status, command_id)),
    }
}

/// Task that writes responses to USB serial
#[embassy_executor::task]
async fn serial_writer_task(
    mut usb_tx: esp_hal::usb_serial_jtag::UsbSerialJtagTx<'static, Async>,
    response_receiver: ResponseReceiver,
) {
    let serialiser = ResponseSerialiser::new();

    loop {
        let (_seq_id, response) = response_receiver.receive().await;
        let encoded = serialiser.serialise(&response);
        let _ = embedded_io_async::Write::write_all(&mut usb_tx, &encoded).await;
    }
}

/// Duration of LED flash in milliseconds
const LED_FLASH_MS: u64 = 50;

/// Task that handles LED flashing without blocking other operations
#[embassy_executor::task]
async fn led_task(mut led: Output<'static>, receiver: LedReceiver) {
    loop {
        // Wait for flash signal
        let flash_duration = receiver.receive().await;
        let duration_ms = match flash_duration {
            LedFlashDuration::Default => LED_FLASH_MS,
            LedFlashDuration::Ms(ms) => ms,
        };

        // Flash LED (turn off then back on, since active low)
        led.set_high(); // LED off
        embassy_time::Timer::after(embassy_time::Duration::from_millis(duration_ms)).await;
        led.set_low(); // LED on
    }
}

/// Task that handles LoRa operations with background listening
///
/// This task continuously listens for incoming LoRa packets and pushes them
/// immediately to the host as unsolicited responses. Commands are processed
/// when available, with a maximum latency of RX_POLL_INTERVAL_MS.
#[embassy_executor::task]
async fn lora_task(
    mut radio: Sx1262Driver<
        Spi<'static, Async>,
        Output<'static>,
        Input<'static>,
        Output<'static>,
        Input<'static>,
    >,
    command_receiver: CommandReceiver,
    response_sender: ResponseSender,
    led_sender: LedSender,
) {
    let dispatcher = CommandDispatcher::new();

    // Initialise LoRa radio
    let _ = radio.init().await;

    loop {
        // Check for pending command (non-blocking)
        match command_receiver.try_receive() {
            Ok(envelope) => {
                // Signal LED flash for TX command (non-blocking)
                let _ = led_sender.try_send(LedFlashDuration::Default);

                let response = dispatcher.dispatch(&mut radio, envelope.command).await;

                // Route response based on source
                match envelope.source {
                    CommandSource::Serial => {
                        response_sender.send((envelope.sequence_id, response)).await;
                    }
                    CommandSource::Ble | CommandSource::WiFi => {
                        // Future: route to appropriate response channel
                    }
                }
            }
            Err(_) => {
                // No command pending - listen for LoRa packets
                match radio.receive(RX_POLL_INTERVAL_MS).await {
                    Ok(packet) => {
                        // Signal LED flash for received packet (non-blocking)
                        let _ = led_sender.try_send(LedFlashDuration::Default);

                        let response = Response::RxPacket {
                            data: packet.data,
                            rssi: packet.rssi,
                            snr: packet.snr,
                        };
                        // Send as unsolicited response
                        response_sender.send((UNSOLICITED_SEQUENCE_ID, response)).await;
                    }
                    Err(LoraError::Timeout) => {
                        // Normal - no packet received within poll interval
                    }
                    Err(_) => {
                        // Other errors - continue listening
                    }
                }
            }
        }
    }
}
