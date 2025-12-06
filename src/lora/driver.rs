//! SX1262 LoRa driver wrapper
//!
//! Wraps the sx1262 crate to implement the LoraRadio trait for use with Embassy.

use crate::config::protocol::MAX_LORA_PAYLOAD;
use crate::config::tcxo;
use crate::lora::traits::{LoraConfig, LoraError, LoraRadio, RxPacket};
use embassy_time::{Duration, Timer};
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal_async::spi::SpiBus;
use heapless::Vec;

/// SX1262 command opcodes
mod cmd {
    pub const SET_STANDBY: u8 = 0x80;
    pub const SET_TX: u8 = 0x83;
    pub const SET_RX: u8 = 0x82;
    pub const SET_RF_FREQUENCY: u8 = 0x86;
    pub const SET_PACKET_TYPE: u8 = 0x8A;
    pub const SET_MODULATION_PARAMS: u8 = 0x8B;
    pub const SET_PACKET_PARAMS: u8 = 0x8C;
    pub const SET_BUFFER_BASE_ADDRESS: u8 = 0x8F;
    pub const SET_PA_CONFIG: u8 = 0x95;
    pub const SET_DIO3_AS_TCXO_CTRL: u8 = 0x97;
    pub const SET_DIO2_AS_RF_SWITCH_CTRL: u8 = 0x9D;
    pub const SET_TX_PARAMS: u8 = 0x8E;
    pub const WRITE_BUFFER: u8 = 0x0E;
    pub const READ_BUFFER: u8 = 0x1E;
    pub const WRITE_REGISTER: u8 = 0x0D;
    pub const GET_RX_BUFFER_STATUS: u8 = 0x13;
    pub const GET_PACKET_STATUS: u8 = 0x14;
    pub const GET_IRQ_STATUS: u8 = 0x12;
    pub const CLEAR_IRQ_STATUS: u8 = 0x02;
    pub const SET_DIO_IRQ_PARAMS: u8 = 0x08;
}

/// SX1262 register addresses
mod reg {
    /// Over-current protection register
    pub const OCP_CONFIGURATION: u16 = 0x08E7;
}

/// Standby modes
mod standby {
    pub const STDBY_RC: u8 = 0x00;
    pub const STDBY_XOSC: u8 = 0x01;
}

/// Packet types
mod packet_type {
    pub const LORA: u8 = 0x01;
}

/// IRQ masks
mod irq {
    pub const TX_DONE: u16 = 0x0001;
    pub const RX_DONE: u16 = 0x0002;
    pub const TIMEOUT: u16 = 0x0200;
    pub const CRC_ERR: u16 = 0x0040;
}

/// Control pins for SX1262
pub struct Sx1262Pins<Nss, Dio1, Nrst, Busy> {
    pub nss: Nss,
    pub dio1: Dio1,
    pub nrst: Nrst,
    pub busy: Busy,
}

/// SX1262 LoRa driver
///
/// Implements the LoraRadio trait using dependency injection for SPI and GPIO pins.
/// Uses SpiBus trait with manual NSS control.
pub struct Sx1262Driver<Spi, Nss, Dio1, Nrst, Busy>
where
    Spi: SpiBus,
    Nss: OutputPin,
    Dio1: InputPin,
    Nrst: OutputPin,
    Busy: InputPin,
{
    spi: Spi,
    nss: Nss,
    dio1: Dio1,
    nrst: Nrst,
    busy: Busy,
    initialised: bool,
    config: Option<LoraConfig>,
}

impl<Spi, Nss, Dio1, Nrst, Busy> Sx1262Driver<Spi, Nss, Dio1, Nrst, Busy>
where
    Spi: SpiBus,
    Nss: OutputPin,
    Dio1: InputPin,
    Nrst: OutputPin,
    Busy: InputPin,
{
    /// Create a new SX1262 driver
    pub fn new(spi: Spi, pins: Sx1262Pins<Nss, Dio1, Nrst, Busy>) -> Self {
        Self {
            spi,
            nss: pins.nss,
            dio1: pins.dio1,
            nrst: pins.nrst,
            busy: pins.busy,
            initialised: false,
            config: None,
        }
    }

    /// Reset the radio
    async fn reset(&mut self) -> Result<(), LoraError> {
        let _ = self.nrst.set_low();
        Timer::after(Duration::from_millis(10)).await;
        let _ = self.nrst.set_high();
        Timer::after(Duration::from_millis(20)).await;
        Ok(())
    }

    /// Wait for the BUSY pin to go low
    async fn wait_not_busy(&mut self) -> Result<(), LoraError> {
        // Poll with timeout
        for _ in 0..1000 {
            if self.busy.is_low().unwrap_or(false) {
                return Ok(());
            }
            Timer::after(Duration::from_micros(100)).await;
        }
        Err(LoraError::BusyTimeout)
    }

    /// Write a command to the radio
    async fn write_command(&mut self, cmd: u8, data: &[u8]) -> Result<(), LoraError> {
        self.wait_not_busy().await?;

        let _ = self.nss.set_low();

        let mut buf = [0u8; 16];
        buf[0] = cmd;
        let len = 1 + data.len().min(15);
        buf[1..len].copy_from_slice(&data[..len - 1]);

        self.spi
            .write(&buf[..len])
            .await
            .map_err(|_| LoraError::SpiError)?;

        let _ = self.nss.set_high();

        Ok(())
    }

    /// Read data from the radio
    async fn read_command(&mut self, cmd: u8, len: usize) -> Result<[u8; 16], LoraError> {
        self.wait_not_busy().await?;

        let _ = self.nss.set_low();

        // SX1262 requires command byte + NOP byte, then reads
        let mut tx_buf = [0u8; 18];
        let mut rx_buf = [0u8; 18];
        tx_buf[0] = cmd;
        tx_buf[1] = 0x00; // NOP

        let total_len = 2 + len;
        self.spi
            .transfer(&mut rx_buf[..total_len], &tx_buf[..total_len])
            .await
            .map_err(|_| LoraError::SpiError)?;

        let _ = self.nss.set_high();

        // Response starts after status byte (index 2)
        let mut result = [0u8; 16];
        result[..len].copy_from_slice(&rx_buf[2..2 + len]);

        Ok(result)
    }

    /// Configure DIO3 as TCXO control
    async fn configure_tcxo(&mut self) -> Result<(), LoraError> {
        // SetDIO3AsTcxoCtrl: voltage code + timeout (24-bit)
        let timeout: u32 = 0x000140; // ~5ms startup time
        let data = [
            tcxo::VOLTAGE_CODE,
            ((timeout >> 16) & 0xFF) as u8,
            ((timeout >> 8) & 0xFF) as u8,
            (timeout & 0xFF) as u8,
        ];
        self.write_command(cmd::SET_DIO3_AS_TCXO_CTRL, &data).await
    }

    /// Configure DIO2 as RF switch control
    async fn configure_dio2_rf_switch(&mut self) -> Result<(), LoraError> {
        self.write_command(cmd::SET_DIO2_AS_RF_SWITCH_CTRL, &[0x01])
            .await
    }

    /// Write to a register
    async fn write_register(&mut self, addr: u16, value: u8) -> Result<(), LoraError> {
        let data = [
            ((addr >> 8) & 0xFF) as u8,
            (addr & 0xFF) as u8,
            value,
        ];
        self.write_command(cmd::WRITE_REGISTER, &data).await
    }

    /// Set current limit (OCP - Over Current Protection)
    /// current_ma: Current limit in mA (default 140mA for SX1262)
    async fn set_current_limit(&mut self, current_ma: u16) -> Result<(), LoraError> {
        // OCP register value = current_ma / 2.5
        // Clamped to valid range
        let ocp_value = ((current_ma as u32 * 10) / 25).min(63) as u8;
        self.write_register(reg::OCP_CONFIGURATION, ocp_value).await
    }

    /// Set standby mode
    async fn set_standby_internal(&mut self) -> Result<(), LoraError> {
        self.write_command(cmd::SET_STANDBY, &[standby::STDBY_RC])
            .await
    }

    /// Set packet type to LoRa
    async fn set_packet_type_lora(&mut self) -> Result<(), LoraError> {
        self.write_command(cmd::SET_PACKET_TYPE, &[packet_type::LORA])
            .await
    }

    /// Set RF frequency
    async fn set_frequency(&mut self, freq_hz: u32) -> Result<(), LoraError> {
        // Frequency = (freq_rf * 2^25) / 32MHz
        let freq_reg = ((freq_hz as u64 * (1 << 25)) / 32_000_000) as u32;
        let data = [
            ((freq_reg >> 24) & 0xFF) as u8,
            ((freq_reg >> 16) & 0xFF) as u8,
            ((freq_reg >> 8) & 0xFF) as u8,
            (freq_reg & 0xFF) as u8,
        ];
        self.write_command(cmd::SET_RF_FREQUENCY, &data).await
    }

    /// Set modulation parameters
    async fn set_modulation_params(&mut self, config: &LoraConfig) -> Result<(), LoraError> {
        let bw = match config.bandwidth_khz {
            7 | 8 => 0x00,   // 7.8 kHz
            10 => 0x08,      // 10.4 kHz
            15 | 16 => 0x01, // 15.6 kHz
            20 | 21 => 0x09, // 20.8 kHz
            31 => 0x02,      // 31.25 kHz
            41 | 42 => 0x0A, // 41.7 kHz
            62 | 63 => 0x03, // 62.5 kHz
            125 => 0x04,     // 125 kHz
            250 => 0x05,     // 250 kHz
            500 => 0x06,     // 500 kHz
            _ => 0x04,       // Default to 125 kHz
        };

        let cr = match config.coding_rate {
            5 => 0x01, // 4/5
            6 => 0x02, // 4/6
            7 => 0x03, // 4/7
            8 => 0x04, // 4/8
            _ => 0x01, // Default to 4/5
        };

        // Low data rate optimisation: required for SF11/SF12 at 125kHz
        let ldro = if config.spreading_factor >= 11 && config.bandwidth_khz <= 125 {
            0x01
        } else {
            0x00
        };

        let data = [config.spreading_factor, bw, cr, ldro];
        self.write_command(cmd::SET_MODULATION_PARAMS, &data).await
    }

    /// Set packet parameters
    async fn set_packet_params(&mut self, payload_len: u8) -> Result<(), LoraError> {
        let data = [
            0x00, 0x08, // Preamble length: 8 symbols
            0x00, // Explicit header
            payload_len,
            0x01, // CRC on
            0x00, // Standard IQ
        ];
        self.write_command(cmd::SET_PACKET_PARAMS, &data).await
    }

    /// Configure the Power Amplifier for SX1262
    /// Must be called before set_tx_power
    async fn configure_pa(&mut self) -> Result<(), LoraError> {
        // SetPaConfig for SX1262 (high power PA)
        // paDutyCycle=0x04, hpMax=0x07, deviceSel=0x00 (SX1262), paLut=0x01
        let data = [0x04, 0x07, 0x00, 0x01];
        self.write_command(cmd::SET_PA_CONFIG, &data).await
    }

    /// Set TX power
    async fn set_tx_power(&mut self, power_dbm: i8) -> Result<(), LoraError> {
        // For SX1262 with HP PA after SetPaConfig(0x04, 0x07, 0x00, 0x01):
        // Power register value maps directly to dBm for range -9 to +22
        // Negative values need to be converted to two's complement
        let power = if power_dbm < 0 {
            (256 + power_dbm as i16) as u8
        } else {
            power_dbm as u8
        };
        let data = [power, 0x04]; // Power, ramp time 200us
        self.write_command(cmd::SET_TX_PARAMS, &data).await
    }

    /// Set buffer base addresses
    async fn set_buffer_base_address(&mut self, tx_base: u8, rx_base: u8) -> Result<(), LoraError> {
        self.write_command(cmd::SET_BUFFER_BASE_ADDRESS, &[tx_base, rx_base])
            .await
    }

    /// Configure IRQ
    async fn configure_irq(&mut self, irq_mask: u16) -> Result<(), LoraError> {
        let data = [
            ((irq_mask >> 8) & 0xFF) as u8,
            (irq_mask & 0xFF) as u8,
            ((irq_mask >> 8) & 0xFF) as u8, // DIO1 mask
            (irq_mask & 0xFF) as u8,
            0x00,
            0x00, // DIO2 mask
            0x00,
            0x00, // DIO3 mask
        ];
        self.write_command(cmd::SET_DIO_IRQ_PARAMS, &data).await
    }

    /// Clear IRQ status
    async fn clear_irq(&mut self, irq_mask: u16) -> Result<(), LoraError> {
        let data = [((irq_mask >> 8) & 0xFF) as u8, (irq_mask & 0xFF) as u8];
        self.write_command(cmd::CLEAR_IRQ_STATUS, &data).await
    }

    /// Get IRQ status
    async fn get_irq_status(&mut self) -> Result<u16, LoraError> {
        let result = self.read_command(cmd::GET_IRQ_STATUS, 2).await?;
        Ok(((result[0] as u16) << 8) | (result[1] as u16))
    }

    /// Write data to TX buffer
    async fn write_buffer(&mut self, offset: u8, data: &[u8]) -> Result<(), LoraError> {
        self.wait_not_busy().await?;

        let _ = self.nss.set_low();

        // Command + offset + data
        let mut buf = [0u8; 258];
        buf[0] = cmd::WRITE_BUFFER;
        buf[1] = offset;
        let len = data.len().min(256);
        buf[2..2 + len].copy_from_slice(&data[..len]);

        self.spi
            .write(&buf[..2 + len])
            .await
            .map_err(|_| LoraError::SpiError)?;

        let _ = self.nss.set_high();

        Ok(())
    }

    /// Read data from RX buffer
    async fn read_buffer(&mut self, offset: u8, len: usize) -> Result<Vec<u8, MAX_LORA_PAYLOAD>, LoraError> {
        self.wait_not_busy().await?;

        let _ = self.nss.set_low();

        // Command + offset + NOP + data
        let mut tx_buf = [0u8; 259];
        let mut rx_buf = [0u8; 259];
        tx_buf[0] = cmd::READ_BUFFER;
        tx_buf[1] = offset;
        tx_buf[2] = 0x00; // NOP

        let total_len = 3 + len;
        self.spi
            .transfer(&mut rx_buf[..total_len], &tx_buf[..total_len])
            .await
            .map_err(|_| LoraError::SpiError)?;

        let _ = self.nss.set_high();

        let mut result = Vec::new();
        result
            .extend_from_slice(&rx_buf[3..3 + len])
            .map_err(|_| LoraError::ReceiveFailed)?;

        Ok(result)
    }

    /// Get RX buffer status
    async fn get_rx_buffer_status(&mut self) -> Result<(u8, u8), LoraError> {
        let result = self.read_command(cmd::GET_RX_BUFFER_STATUS, 2).await?;
        Ok((result[0], result[1])) // (payload_length, buffer_offset)
    }

    /// Get packet status
    async fn get_packet_status(&mut self) -> Result<(i16, i8), LoraError> {
        let result = self.read_command(cmd::GET_PACKET_STATUS, 3).await?;

        // RSSI: -result[0]/2
        let rssi = -(result[0] as i16) / 2;

        // SNR: result[1] as signed / 4
        let snr = (result[1] as i8) / 4;

        Ok((rssi, snr))
    }

    /// Wait for DIO1 interrupt with timeout
    async fn wait_for_irq(&mut self, timeout_ms: u32) -> Result<u16, LoraError> {
        let deadline = embassy_time::Instant::now() + Duration::from_millis(timeout_ms as u64);

        loop {
            // Check if DIO1 is high (interrupt pending)
            if self.dio1.is_high().unwrap_or(false) {
                return self.get_irq_status().await;
            }

            if embassy_time::Instant::now() >= deadline {
                return Err(LoraError::Timeout);
            }

            Timer::after(Duration::from_micros(100)).await;
        }
    }

    /// Start continuous receive mode (like Arduino's startReceive)
    /// Puts the radio into RX mode with no timeout
    async fn start_receive_mode(&mut self) -> Result<(), LoraError> {
        // Set to standby first
        self.set_standby_internal().await?;

        // Set packet parameters for max length
        self.set_packet_params(MAX_LORA_PAYLOAD as u8).await?;

        // Configure IRQ for RX done, timeout, CRC error
        self.configure_irq(irq::RX_DONE | irq::TIMEOUT | irq::CRC_ERR)
            .await?;
        self.clear_irq(0xFFFF).await?;

        // Start continuous RX (timeout = 0xFFFFFF means continuous)
        let timeout_bytes = [0xFF, 0xFF, 0xFF];
        self.write_command(cmd::SET_RX, &timeout_bytes).await?;

        Ok(())
    }
}

impl<Spi, Nss, Dio1, Nrst, Busy> LoraRadio for Sx1262Driver<Spi, Nss, Dio1, Nrst, Busy>
where
    Spi: SpiBus,
    Nss: OutputPin,
    Dio1: InputPin,
    Nrst: OutputPin,
    Busy: InputPin,
{
    async fn init(&mut self) -> Result<(), LoraError> {
        // Reset the radio
        self.reset().await?;
        self.wait_not_busy().await?;

        // Set standby mode
        self.set_standby_internal().await?;

        // Configure TCXO (1.8V)
        self.configure_tcxo().await?;
        Timer::after(Duration::from_millis(10)).await;

        // Configure DIO2 as RF switch control
        self.configure_dio2_rf_switch().await?;

        // Set current limit (140mA as per Arduino config)
        self.set_current_limit(140).await?;

        // Set packet type to LoRa
        self.set_packet_type_lora().await?;

        // Set buffer base addresses
        self.set_buffer_base_address(0x00, 0x80).await?;

        // Apply default configuration
        self.configure(&LoraConfig::default()).await?;

        // Start in receive mode (like Arduino's startReceive at end of begin())
        self.start_receive_mode().await?;

        self.initialised = true;
        Ok(())
    }

    async fn transmit(&mut self, data: &[u8]) -> Result<(), LoraError> {
        if !self.initialised {
            return Err(LoraError::NotInitialised);
        }

        if data.is_empty() || data.len() > MAX_LORA_PAYLOAD {
            return Err(LoraError::InvalidConfig);
        }

        // Set to standby
        self.set_standby_internal().await?;

        // Set packet parameters with payload length
        self.set_packet_params(data.len() as u8).await?;

        // Write data to buffer
        self.write_buffer(0x00, data).await?;

        // Configure IRQ for TX done
        self.configure_irq(irq::TX_DONE).await?;
        self.clear_irq(0xFFFF).await?;

        // Start transmission (timeout 0 = no timeout)
        self.write_command(cmd::SET_TX, &[0x00, 0x00, 0x00]).await?;

        // Wait for TX done (10 second timeout)
        let irq_status = self.wait_for_irq(10000).await?;

        // Clear IRQ
        self.clear_irq(0xFFFF).await?;

        // Return to RX mode (like Arduino's startReceive after transmit)
        self.start_receive_mode().await?;

        if irq_status & irq::TX_DONE != 0 {
            Ok(())
        } else {
            Err(LoraError::TransmitFailed)
        }
    }

    async fn receive(&mut self, timeout_ms: u32) -> Result<RxPacket, LoraError> {
        if !self.initialised {
            return Err(LoraError::NotInitialised);
        }

        // Set to standby
        self.set_standby_internal().await?;

        // Set packet parameters (max length, will read actual from header)
        self.set_packet_params(MAX_LORA_PAYLOAD as u8).await?;

        // Configure IRQ for RX done, timeout, CRC error
        self.configure_irq(irq::RX_DONE | irq::TIMEOUT | irq::CRC_ERR)
            .await?;
        self.clear_irq(0xFFFF).await?;

        // Calculate SX1262 timeout value
        // Timeout = timeout_value * 15.625 us
        let timeout_val = if timeout_ms == 0 {
            0x000000 // No timeout (continuous RX)
        } else {
            let us = timeout_ms as u32 * 1000;
            let val = us / 16; // Approximate 15.625us
            val.min(0xFFFFFF)
        };

        let timeout_bytes = [
            ((timeout_val >> 16) & 0xFF) as u8,
            ((timeout_val >> 8) & 0xFF) as u8,
            (timeout_val & 0xFF) as u8,
        ];

        // Start reception
        self.write_command(cmd::SET_RX, &timeout_bytes).await?;

        // Wait for RX done or timeout
        let irq_status = self.wait_for_irq(timeout_ms + 1000).await?;

        // Clear IRQ
        self.clear_irq(0xFFFF).await?;

        // Check for timeout
        if irq_status & irq::TIMEOUT != 0 {
            return Err(LoraError::Timeout);
        }

        // Check for CRC error
        if irq_status & irq::CRC_ERR != 0 {
            return Err(LoraError::CrcError);
        }

        // Check for RX done
        if irq_status & irq::RX_DONE == 0 {
            return Err(LoraError::ReceiveFailed);
        }

        // Get buffer status
        let (payload_len, buffer_offset) = self.get_rx_buffer_status().await?;

        // Read data
        let data = self.read_buffer(buffer_offset, payload_len as usize).await?;

        // Get packet status
        let (rssi, snr) = self.get_packet_status().await?;

        Ok(RxPacket { data, rssi, snr })
    }

    async fn configure(&mut self, config: &LoraConfig) -> Result<(), LoraError> {
        // Set to standby before configuration
        self.set_standby_internal().await?;

        // Set frequency
        self.set_frequency(config.frequency_hz).await?;

        // Set modulation parameters
        self.set_modulation_params(config).await?;

        // Configure Power Amplifier (must be called before SetTxParams)
        self.configure_pa().await?;

        // Set TX power
        self.set_tx_power(config.tx_power_dbm).await?;

        self.config = Some(config.clone());

        Ok(())
    }

    async fn set_standby(&mut self) -> Result<(), LoraError> {
        self.set_standby_internal().await
    }
}
