//! LoRa radio trait for abstraction and testability
//!
//! This trait defines the interface for LoRa radio operations,
//! allowing the actual hardware driver to be swapped with a mock for testing.

use crate::config::protocol::MAX_LORA_PAYLOAD;
use core::future::Future;
use heapless::Vec;

/// Errors that can occur during LoRa operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoraError {
    /// Operation timed out
    Timeout,
    /// CRC error in received packet
    CrcError,
    /// Transmission failed
    TransmitFailed,
    /// Reception failed
    ReceiveFailed,
    /// Invalid configuration
    InvalidConfig,
    /// Radio busy timeout
    BusyTimeout,
    /// SPI communication error
    SpiError,
    /// Radio not initialised
    NotInitialised,
}

/// LoRa sync word selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncWord {
    /// Private networks (SX1262 power-on default, register value 0x1424).
    #[default]
    Private,
    /// Public networks / LoRaWAN (register value 0x3444).
    #[allow(dead_code)] // constructed by the gateway mode
    Public,
}

impl SyncWord {
    /// The (MSB, LSB) register pair for registers 0x0740/0x0741.
    pub fn register_bytes(self) -> (u8, u8) {
        match self {
            SyncWord::Private => (0x14, 0x24),
            SyncWord::Public => (0x34, 0x44),
        }
    }
}

/// Configuration for LoRa modulation
#[derive(Debug, Clone)]
pub struct LoraConfig {
    /// Centre frequency in Hz
    pub frequency_hz: u32,
    /// Spreading factor (7-12)
    pub spreading_factor: u8,
    /// Bandwidth in kHz (7.8, 10.4, 15.6, 20.8, 31.25, 41.7, 62.5, 125, 250, 500)
    pub bandwidth_khz: u32,
    /// Coding rate denominator (5-8 for 4/5 to 4/8)
    pub coding_rate: u8,
    /// Transmit power in dBm
    pub tx_power_dbm: i8,
    /// Network sync word
    pub sync_word: SyncWord,
    /// Inverted IQ (LoRaWAN downlinks); standard IQ otherwise
    pub iq_inverted: bool,
    /// Append/verify the payload CRC (LoRaWAN downlinks disable it)
    pub crc_on: bool,
    /// Preamble length in symbols
    pub preamble_len: u16,
}

impl Default for LoraConfig {
    fn default() -> Self {
        use crate::config::lora_defaults;

        Self {
            frequency_hz: lora_defaults::FREQUENCY_HZ,
            spreading_factor: lora_defaults::SPREADING_FACTOR,
            bandwidth_khz: lora_defaults::BANDWIDTH_KHZ,
            coding_rate: lora_defaults::CODING_RATE,
            tx_power_dbm: lora_defaults::TX_POWER_DBM,
            sync_word: SyncWord::Private,
            iq_inverted: false,
            crc_on: true,
            preamble_len: 8,
        }
    }
}

/// Received packet with metadata
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RxPacket {
    /// Received data
    pub data: Vec<u8, MAX_LORA_PAYLOAD>,
    /// Received Signal Strength Indicator in dBm
    pub rssi: i16,
    /// Signal-to-Noise Ratio in dB
    pub snr: i8,
}

/// Abstract LoRa radio interface for testability
///
/// This trait allows the dispatcher to work with either the real SX1262
/// hardware driver or a mock implementation for testing.
pub trait LoraRadio {
    /// Initialise the radio hardware
    fn init(&mut self) -> impl Future<Output = Result<(), LoraError>>;

    /// Transmit data over LoRa
    ///
    /// Blocks until transmission is complete or an error occurs.
    fn transmit(&mut self, data: &[u8]) -> impl Future<Output = Result<(), LoraError>>;

    /// Ensure the radio is listening in continuous RX.
    ///
    /// Involves SPI traffic, so it is NOT cancel-safe: never race it in a
    /// select. A no-op when the radio is already armed.
    fn arm_receive(&mut self) -> impl Future<Output = Result<(), LoraError>>;

    /// Wait until the radio signals a pending RX event or the timeout expires.
    ///
    /// Returns the instant the event was observed, captured before any SPI
    /// traffic - this timestamp anchors LoRaWAN downlink windows, so read
    /// latency must not pollute it.
    ///
    /// Cancel-safe: implementations must not touch the SPI bus here, so the
    /// future can be raced in a select and dropped at any await point.
    fn wait_rx_event(
        &mut self,
        timeout_ms: u32,
    ) -> impl Future<Output = Result<embassy_time::Instant, LoraError>>;

    /// Read the packet behind a signalled RX event.
    ///
    /// Call after `wait_rx_event` succeeds. Involves SPI traffic, so it is
    /// NOT cancel-safe: never race it in a select.
    fn read_packet(&mut self) -> impl Future<Output = Result<RxPacket, LoraError>>;

    /// Configure the radio parameters
    fn configure(&mut self, config: &LoraConfig) -> impl Future<Output = Result<(), LoraError>>;

    /// Set the radio to standby mode
    #[allow(dead_code)]
    fn set_standby(&mut self) -> impl Future<Output = Result<(), LoraError>>;
}

#[cfg(test)]
pub mod mock {
    //! Mock LoRa radio for testing

    use super::*;
    use core::cell::RefCell;

    /// Mock LoRa radio for unit testing
    pub struct MockLoraRadio {
        /// Packets queued to be returned by receive()
        rx_queue: RefCell<Vec<RxPacket, 8>>,
        /// Record of transmitted packets
        tx_history: RefCell<Vec<Vec<u8, MAX_LORA_PAYLOAD>, 8>>,
        /// Current configuration
        config: RefCell<Option<LoraConfig>>,
        /// Error to return on next transmit
        next_tx_error: RefCell<Option<LoraError>>,
        /// Error to return on next receive
        next_rx_error: RefCell<Option<LoraError>>,
        /// Error to return on next configure
        next_configure_error: RefCell<Option<LoraError>>,
        /// Whether init has been called
        initialised: RefCell<bool>,
    }

    impl MockLoraRadio {
        /// Create a new mock radio
        pub fn new() -> Self {
            Self {
                rx_queue: RefCell::new(Vec::new()),
                tx_history: RefCell::new(Vec::new()),
                config: RefCell::new(None),
                next_tx_error: RefCell::new(None),
                next_rx_error: RefCell::new(None),
                next_configure_error: RefCell::new(None),
                initialised: RefCell::new(false),
            }
        }

        /// Queue a packet to be returned by the next receive() call
        pub fn queue_rx_packet(&self, packet: RxPacket) {
            let _ = self.rx_queue.borrow_mut().push(packet);
        }

        /// Set an error to be returned by the next transmit() call
        pub fn set_next_tx_error(&self, error: LoraError) {
            *self.next_tx_error.borrow_mut() = Some(error);
        }

        /// Set an error to be returned by the next receive() call
        pub fn set_next_rx_error(&self, error: LoraError) {
            *self.next_rx_error.borrow_mut() = Some(error);
        }

        /// Set an error to be returned by the next configure() call
        pub fn set_next_configure_error(&self, error: LoraError) {
            *self.next_configure_error.borrow_mut() = Some(error);
        }

        /// Get all transmitted packets
        pub fn get_tx_history(&self) -> Vec<Vec<u8, MAX_LORA_PAYLOAD>, 8> {
            self.tx_history.borrow().clone()
        }

        /// Check if the radio has been initialised
        pub fn is_initialised(&self) -> bool {
            *self.initialised.borrow()
        }

        /// Get the current configuration
        pub fn get_config(&self) -> Option<LoraConfig> {
            self.config.borrow().clone()
        }
    }

    impl Default for MockLoraRadio {
        fn default() -> Self {
            Self::new()
        }
    }

    impl LoraRadio for MockLoraRadio {
        async fn init(&mut self) -> Result<(), LoraError> {
            *self.initialised.borrow_mut() = true;
            Ok(())
        }

        async fn transmit(&mut self, data: &[u8]) -> Result<(), LoraError> {
            if let Some(error) = self.next_tx_error.borrow_mut().take() {
                return Err(error);
            }

            let mut packet = Vec::new();
            packet
                .extend_from_slice(data)
                .map_err(|_| LoraError::TransmitFailed)?;
            let _ = self.tx_history.borrow_mut().push(packet);

            Ok(())
        }

        async fn arm_receive(&mut self) -> Result<(), LoraError> {
            Ok(())
        }

        async fn wait_rx_event(&mut self, _timeout_ms: u32) -> Result<embassy_time::Instant, LoraError> {
            // An event is pending when a packet or an injected error waits.
            if self.next_rx_error.borrow().is_some() || !self.rx_queue.borrow().is_empty() {
                Ok(embassy_time::Instant::now())
            } else {
                Err(LoraError::Timeout)
            }
        }

        async fn read_packet(&mut self) -> Result<RxPacket, LoraError> {
            if let Some(error) = self.next_rx_error.borrow_mut().take() {
                return Err(error);
            }

            // Pop from front (FIFO order)
            let mut queue = self.rx_queue.borrow_mut();
            if queue.is_empty() {
                return Err(LoraError::ReceiveFailed);
            }
            Ok(queue.remove(0))
        }

        async fn configure(&mut self, config: &LoraConfig) -> Result<(), LoraError> {
            if let Some(error) = self.next_configure_error.borrow_mut().take() {
                return Err(error);
            }
            *self.config.borrow_mut() = Some(config.clone());
            Ok(())
        }

        async fn set_standby(&mut self) -> Result<(), LoraError> {
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_mock_transmit() {
            let mut radio = MockLoraRadio::new();

            // Use a simple blocking executor for testing
            futures::executor::block_on(async {
                radio.init().await.unwrap();

                let data = [0x01, 0x02, 0x03];
                radio.transmit(&data).await.unwrap();

                let history = radio.get_tx_history();
                assert_eq!(history.len(), 1);
                assert_eq!(history[0].as_slice(), &data);
            });
        }

        #[test]
        fn test_mock_receive_queued() {
            let mut radio = MockLoraRadio::new();

            futures::executor::block_on(async {
                let mut data = Vec::new();
                data.extend_from_slice(&[0x48, 0x65, 0x6C, 0x6C, 0x6F])
                    .unwrap();

                radio.queue_rx_packet(RxPacket {
                    data: data.clone(),
                    rssi: -50,
                    snr: 10,
                });

                radio.arm_receive().await.unwrap();
                radio.wait_rx_event(1000).await.unwrap();
                let packet = radio.read_packet().await.unwrap();
                assert_eq!(packet.data.as_slice(), data.as_slice());
                assert_eq!(packet.rssi, -50);
                assert_eq!(packet.snr, 10);
            });
        }

        #[test]
        fn test_mock_receive_timeout() {
            let mut radio = MockLoraRadio::new();

            futures::executor::block_on(async {
                let result = radio.wait_rx_event(1000).await;
                assert_eq!(result, Err(LoraError::Timeout));
            });
        }

        #[test]
        fn test_mock_tx_error() {
            let mut radio = MockLoraRadio::new();

            futures::executor::block_on(async {
                radio.set_next_tx_error(LoraError::TransmitFailed);

                let result = radio.transmit(&[0x01]).await;
                assert_eq!(result, Err(LoraError::TransmitFailed));

                // Error should be cleared, next call should succeed
                radio.transmit(&[0x02]).await.unwrap();
            });
        }
    }
}
