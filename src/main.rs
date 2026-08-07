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
use embassy_net::{Runner, Stack, StackResources};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::UsbDevice;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::ram;
use esp_hal::otg_fs::asynch::{Config as DriverConfig, Driver};
use esp_hal::otg_fs::Usb;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::Async;
use static_cell::StaticCell;

mod bridge;
mod config;
mod debug;
mod dispatcher;
mod hub_config;
mod lora;
mod net;
mod tasks;
mod usb;

use dispatcher::COMMAND_CHANNEL;
use hub_config::store::ConfigStore;
use hub_config::HubConfig;
use lora::driver::{Sx1262Driver, Sx1262Pins};
use tasks::{AdminReceiver, CommandReceiver, CommandSender, LedReceiver, LedSender, ADMIN_CHANNEL, LED_CHANNEL};

/// Static executor for embassy
static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();

/// Static cell for esp-radio controller (needed for 'static lifetime)
static RADIO_CONTROLLER: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();

/// Active configuration, loaded once at boot (flash overrides baked defaults)
static HUB_CONFIG: StaticCell<HubConfig> = StaticCell::new();

/// Socket storage for the embassy-net stack: MQTT TCP + DNS + DHCP + spare
static STACK_RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();

// USB static buffers (must be 'static for embassy-usb)
static EP_OUT_BUFFER: StaticCell<[u8; 1024]> = StaticCell::new();
static DATA_CDC_STATE: StaticCell<State<'static>> = StaticCell::new();
static DEBUG_CDC_STATE: StaticCell<State<'static>> = StaticCell::new();
static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
/// Backing store for the USB serial string, which embassy-usb borrows for the
/// device's lifetime ("WTH-" plus the 3-byte device id as hex).
static USB_SERIAL: StaticCell<[u8; 10]> = StaticCell::new();

#[esp_hal::main]
fn main() -> ! {
    // Heap for the radio subsystem (WiFi driver buffers must live in internal
    // RAM); the bootloader-reclaimed region doubles it for socket buffers.
    esp_alloc::heap_allocator!(size: 64 * 1024);
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));

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

    // Read unique device ID from eFuse MAC address (last 3 bytes). Used for the
    // USB serial and the MQTT client id so each board is distinct.
    let mac = esp_hal::efuse::Efuse::read_base_mac_address();
    let device_id: [u8; 3] = [mac[3], mac[4], mac[5]];
    let usb_serial = format_usb_serial(USB_SERIAL.init([0u8; 10]), device_id);

    // Configure USB OTG with dual CDC-ACM (data + debug ports)
    let usb = Usb::new(peripherals.USB0, peripherals.GPIO20, peripherals.GPIO19);

    // Initialise static buffers
    let ep_out_buffer = EP_OUT_BUFFER.init([0u8; 1024]);
    let data_cdc_state = DATA_CDC_STATE.init(State::new());
    let debug_cdc_state = DEBUG_CDC_STATE.init(State::new());
    let config_descriptor = CONFIG_DESCRIPTOR.init([0u8; 256]);
    let bos_descriptor = BOS_DESCRIPTOR.init([0u8; 256]);
    let control_buf = CONTROL_BUF.init([0u8; 64]);

    // Create USB driver
    let driver = Driver::new(usb, ep_out_buffer, DriverConfig::default());

    // Build USB device with dual CDC-ACM
    let mut usb_config = embassy_usb::Config::new(0x303A, 0x1001);
    usb_config.manufacturer = Some("Walkie-Textie");
    usb_config.product = Some("Walkie-Textie Hub");
    usb_config.serial_number = Some(usb_serial);
    usb_config.max_power = 100;
    usb_config.max_packet_size_0 = 64;

    let mut builder = embassy_usb::Builder::new(
        driver,
        usb_config,
        config_descriptor,
        bos_descriptor,
        &mut [],
        control_buf,
    );

    // Create CDC classes (data port first, then debug port)
    let data_cdc = CdcAcmClass::new(&mut builder, data_cdc_state, 64);
    let debug_cdc = CdcAcmClass::new(&mut builder, debug_cdc_state, 64);

    // Build the USB device
    let usb_device = builder.build();

    // Initialise esp-radio (must be after esp_rtos::start); the WiFi driver
    // borrows this controller.
    let radio_controller = RADIO_CONTROLLER.init(
        esp_radio::init().expect("Failed to initialize esp-radio")
    );

    let (wifi_controller, interfaces) = esp_radio::wifi::new(
        radio_controller,
        peripherals.WIFI,
        esp_radio::wifi::Config::default(),
    )
    .expect("Failed to initialize WiFi");

    // Network stack over the station interface, DHCP-configured
    let rng = esp_hal::rng::Rng::new();
    let net_seed = ((rng.random() as u64) << 32) | rng.random() as u64;
    let (stack, net_runner) = embassy_net::new(
        interfaces.sta,
        embassy_net::Config::dhcpv4(Default::default()),
        STACK_RESOURCES.init(StackResources::new()),
        net_seed,
    );

    // Open the flash config store (nvs partition). The hub still runs on
    // baked defaults without it; settings just cannot be persisted.
    let config_store = ConfigStore::new(peripherals.FLASH).ok();

    let board = Board {
        usb_device,
        data_cdc,
        debug_cdc,
        lora_driver,
        led,
        device_id,
        config_store,
        wifi_controller,
        net_runner,
        stack,
    };

    // Create and run the embassy executor
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner| {
        spawner.must_spawn(async_main(spawner, board));
    })
}

/// Render the USB serial as `WTH-XXXXXX` from the 3-byte device id, writing into
/// the caller-owned buffer so it can outlive `main` for the USB descriptor.
fn format_usb_serial(buf: &'static mut [u8; 10], device_id: [u8; 3]) -> &'static str {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    buf[0] = b'W';
    buf[1] = b'T';
    buf[2] = b'H';
    buf[3] = b'-';
    for (i, byte) in device_id.iter().enumerate() {
        buf[4 + i * 2] = HEX[(byte >> 4) as usize];
        buf[5 + i * 2] = HEX[(byte & 0x0F) as usize];
    }
    // Only ASCII was written, so this never falls back.
    core::str::from_utf8(buf).unwrap_or("WTH-000000")
}

/// Type alias for the USB driver
type UsbDriver = Driver<'static>;

/// Type alias for the CDC class
type CdcClass = CdcAcmClass<'static, UsbDriver>;

/// Type alias for the concrete LoRa driver on this board
type LoraDriver = Sx1262Driver<
    Spi<'static, Async>,
    Output<'static>,
    Input<'static>,
    Output<'static>,
    Input<'static>,
>;

/// Type alias for the WiFi station device inside the net stack runner
type NetRunner = Runner<'static, esp_radio::wifi::WifiDevice<'static>>;

/// Everything constructed in `main` that the async side takes ownership of
struct Board {
    usb_device: UsbDevice<'static, UsbDriver>,
    data_cdc: CdcClass,
    debug_cdc: CdcClass,
    lora_driver: LoraDriver,
    led: Output<'static>,
    device_id: [u8; 3],
    config_store: Option<ConfigStore>,
    wifi_controller: esp_radio::wifi::WifiController<'static>,
    net_runner: NetRunner,
    stack: Stack<'static>,
}

#[embassy_executor::task]
async fn async_main(spawner: Spawner, board: Board) {
    let Board {
        usb_device,
        data_cdc,
        debug_cdc,
        lora_driver,
        led,
        device_id,
        mut config_store,
        wifi_controller,
        net_runner,
        stack,
    } = board;

    // Load the active config: a stored value overrides the baked defaults.
    let stored = match config_store.as_mut() {
        Some(store) => store.load().await,
        None => None,
    };
    let hub_config: &'static HubConfig = HUB_CONFIG.init(hub_config::select_config(stored));

    // Get channel handles
    let command_sender = COMMAND_CHANNEL.sender();
    let command_receiver = COMMAND_CHANNEL.receiver();
    let led_sender = LED_CHANNEL.sender();
    let led_receiver = LED_CHANNEL.receiver();
    let admin_receiver = ADMIN_CHANNEL.receiver();

    // Split CDC classes into sender/receiver and wrap for embedded_io_async
    let (data_tx, data_rx) = data_cdc.split();
    let (debug_tx, _debug_rx) = debug_cdc.split();
    let data_reader = usb::CdcReader::new(data_rx);
    let data_writer = usb::CdcWriter::new(data_tx);

    // Spawn USB device task (must run to handle USB events)
    spawner.spawn(usb_device_wrapper(usb_device)).unwrap();

    // Spawn serial tasks using data CDC
    spawner.spawn(serial_reader_wrapper(data_reader, command_sender)).unwrap();
    spawner.spawn(serial_writer_wrapper(data_writer)).unwrap();

    // Spawn debug writer task
    spawner.spawn(debug_writer_wrapper(debug_tx)).unwrap();

    // Log startup message
    debug!("Walkie-Textie Hub v{}.{}.{} starting...",
        crate::config::protocol::VERSION_MAJOR,
        crate::config::protocol::VERSION_MINOR,
        crate::config::protocol::VERSION_PATCH
    );
    debug!("Device ID: {:02X}{:02X}{:02X}", device_id[0], device_id[1], device_id[2]);

    // Spawn other tasks
    debug!("Starting tasks...");
    spawner.spawn(admin_wrapper(admin_receiver)).unwrap();
    spawner.spawn(lora_wrapper(lora_driver, command_receiver, led_sender)).unwrap();
    spawner.spawn(led_wrapper(led, led_receiver)).unwrap();
    spawner.spawn(hub_ctrl_wrapper(config_store, hub_config)).unwrap();
    spawner.spawn(net_runner_wrapper(net_runner)).unwrap();
    spawner.spawn(wifi_wrapper(wifi_controller, hub_config)).unwrap();
    spawner.spawn(net_watch_wrapper(stack)).unwrap();
    spawner.spawn(bridge_wrapper()).unwrap();
    spawner.spawn(mqtt_wrapper(stack, hub_config, device_id)).unwrap();
    debug!("All tasks started");
}

/// Wrapper task for the LoRa <-> MQTT bridge
#[embassy_executor::task]
async fn bridge_wrapper() {
    tasks::bridge_task().await;
}

/// Wrapper task for the MQTT client
#[embassy_executor::task]
async fn mqtt_wrapper(stack: Stack<'static>, config: &'static HubConfig, device_id: [u8; 3]) {
    tasks::mqtt_task(stack, config, device_id).await;
}

/// Wrapper task for hub provisioning/status commands
#[embassy_executor::task]
async fn hub_ctrl_wrapper(store: Option<ConfigStore>, config: &'static HubConfig) {
    tasks::hub_ctrl_task(store, config).await;
}

/// Wrapper task for the embassy-net stack runner
#[embassy_executor::task]
async fn net_runner_wrapper(mut runner: NetRunner) {
    runner.run().await;
}

/// Wrapper task for the WiFi connection manager
#[embassy_executor::task]
async fn wifi_wrapper(
    controller: esp_radio::wifi::WifiController<'static>,
    config: &'static HubConfig,
) {
    tasks::wifi_task(controller, config).await;
}

/// Wrapper task for the DHCP/IP state watcher
#[embassy_executor::task]
async fn net_watch_wrapper(stack: Stack<'static>) {
    tasks::net_watch_task(stack).await;
}

/// Wrapper task for USB device (handles USB events)
#[embassy_executor::task]
async fn usb_device_wrapper(mut usb: UsbDevice<'static, UsbDriver>) {
    usb.run().await;
}

/// Wrapper task for serial reader
#[embassy_executor::task]
async fn serial_reader_wrapper(
    reader: usb::CdcReader<'static, UsbDriver>,
    command_sender: CommandSender,
) {
    tasks::serial_reader_task(reader, command_sender).await;
}

/// Wrapper task for serial writer
#[embassy_executor::task]
async fn serial_writer_wrapper(writer: usb::CdcWriter<'static, UsbDriver>) {
    tasks::serial_writer_task(writer).await;
}

/// Wrapper task for debug output
#[embassy_executor::task]
async fn debug_writer_wrapper(debug_tx: embassy_usb::class::cdc_acm::Sender<'static, UsbDriver>) {
    debug::debug_writer_task(debug_tx).await;
}

/// Wrapper task for admin commands (reboot, etc.)
#[embassy_executor::task]
async fn admin_wrapper(receiver: AdminReceiver) {
    tasks::admin_task(receiver).await;
}

/// Wrapper task for LED control
#[embassy_executor::task]
async fn led_wrapper(led: Output<'static>, receiver: LedReceiver) {
    tasks::led_task(led, receiver).await;
}

/// Wrapper task for LoRa operations
#[embassy_executor::task]
async fn lora_wrapper(
    radio: LoraDriver,
    command_receiver: CommandReceiver,
    led_sender: LedSender,
) {
    tasks::lora_task(radio, command_receiver, led_sender).await;
}
