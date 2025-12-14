#![no_std]
#![no_main]

extern crate alloc;

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
use esp_backtrace as _;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_hal::Async;
use static_cell::StaticCell;

mod ble;
mod commands;
mod config;
mod dispatcher;
mod lora;
mod protocol;
mod serial;
mod tasks;

use dispatcher::COMMAND_CHANNEL;
use lora::driver::{Sx1262Driver, Sx1262Pins};
use tasks::{CommandReceiver, CommandSender, LedReceiver, LedSender, LED_CHANNEL};

/// Static executor for embassy
static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();

/// Static cell for esp-radio controller (needed for 'static lifetime)
static RADIO_CONTROLLER: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();

#[esp_hal::main]
fn main() -> ! {
    // Initialise heap allocator for BLE support (64KB - BLE requires significant heap)
    esp_alloc::heap_allocator!(size: 64 * 1024);

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

    // Read unique device ID from eFuse MAC address (last 3 bytes)
    let mac = esp_hal::efuse::Efuse::read_base_mac_address();
    let device_id: [u8; 3] = [mac[3], mac[4], mac[5]];

    // Initialise esp-radio for BLE support (must be after esp_rtos::start)
    let radio_controller = RADIO_CONTROLLER.init(
        esp_radio::init().expect("Failed to initialize esp-radio")
    );

    // Create BLE connector (ownership is passed to ExternalController)
    let ble_connector = esp_radio::ble::controller::BleConnector::new(
        radio_controller,
        peripherals.BT,
        esp_radio::ble::Config::default(),
    ).expect("Failed to initialize BLE connector");

    // Wrap in ExternalController for trouble-host compatibility
    let controller: trouble_host::prelude::ExternalController<_, 10> =
        trouble_host::prelude::ExternalController::new(ble_connector);

    // Create and run the embassy executor
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner| {
        spawner.must_spawn(async_main(spawner, usb_rx, usb_tx, lora_driver, led, controller, device_id));
    })
}

/// Type alias for the BLE controller
type BleController = trouble_host::prelude::ExternalController<
    esp_radio::ble::controller::BleConnector<'static>,
    10,
>;

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
    ble_controller: BleController,
    device_id: [u8; 3],
) {
    // Get channel handles
    let command_sender = COMMAND_CHANNEL.sender();
    let command_receiver = COMMAND_CHANNEL.receiver();
    let led_sender = LED_CHANNEL.sender();
    let led_receiver = LED_CHANNEL.receiver();

    // Spawn tasks
    spawner.spawn(serial_reader_wrapper(usb_rx, command_sender)).unwrap();
    spawner.spawn(serial_writer_wrapper(usb_tx)).unwrap();
    spawner.spawn(lora_wrapper(lora_driver, command_receiver, led_sender)).unwrap();
    spawner.spawn(led_wrapper(led, led_receiver)).unwrap();
    spawner.spawn(ble_wrapper(ble_controller, device_id)).unwrap();
}

/// Wrapper task for serial reader
#[embassy_executor::task]
async fn serial_reader_wrapper(
    usb_rx: esp_hal::usb_serial_jtag::UsbSerialJtagRx<'static, Async>,
    command_sender: CommandSender,
) {
    tasks::serial_reader_task(usb_rx, command_sender).await;
}

/// Wrapper task for serial writer
#[embassy_executor::task]
async fn serial_writer_wrapper(usb_tx: esp_hal::usb_serial_jtag::UsbSerialJtagTx<'static, Async>) {
    tasks::serial_writer_task(usb_tx).await;
}

/// Wrapper task for LED control
#[embassy_executor::task]
async fn led_wrapper(led: Output<'static>, receiver: LedReceiver) {
    tasks::led_task(led, receiver).await;
}

/// Wrapper task for BLE connectivity
#[embassy_executor::task]
async fn ble_wrapper(controller: BleController, device_id: [u8; 3]) {
    tasks::ble_task(controller, device_id).await;
}

/// Wrapper task for LoRa operations
#[embassy_executor::task]
async fn lora_wrapper(
    radio: Sx1262Driver<
        Spi<'static, Async>,
        Output<'static>,
        Input<'static>,
        Output<'static>,
        Input<'static>,
    >,
    command_receiver: CommandReceiver,
    led_sender: LedSender,
) {
    tasks::lora_task(radio, command_receiver, led_sender).await;
}
