//! SX1262 LoRa driver wrapper
//!
//! Wraps the sx1262 crate to implement the LoraRadio trait for use with Embassy.

use crate::config::protocol::MAX_LORA_PAYLOAD;
use crate::config::tcxo;
use crate::lora::calibration::{image_cal_params, ldro_enabled, CALIBRATE_ALL};
use crate::lora::traits::{LoraConfig, LoraError, LoraRadio, RxPacket};
use embassy_time::{with_timeout, Duration, Timer};
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal_async::digital::Wait;
use embedded_hal_async::spi::SpiBus;
use heapless::Vec;

/// SX1262 command opcodes
mod cmd {
    pub const SET_STANDBY: u8 = 0x80;
    pub const SET_TX: u8 = 0x83;
    pub const SET_RX: u8 = 0x82;
    pub const SET_REGULATOR_MODE: u8 = 0x96;
    pub const CLEAR_DEVICE_ERRORS: u8 = 0x07;
    pub const READ_REGISTER: u8 = 0x1D;
    pub const SET_RF_FREQUENCY: u8 = 0x86;
    pub const SET_PACKET_TYPE: u8 = 0x8A;
    pub const SET_MODULATION_PARAMS: u8 = 0x8B;
    pub const SET_PACKET_PARAMS: u8 = 0x8C;
    pub const SET_BUFFER_BASE_ADDRESS: u8 = 0x8F;
    pub const SET_PA_CONFIG: u8 = 0x95;
    pub const SET_DIO3_AS_TCXO_CTRL: u8 = 0x97;
    pub const SET_DIO2_AS_RF_SWITCH_CTRL: u8 = 0x9D;
    pub const CALIBRATE: u8 = 0x89;
    pub const CALIBRATE_IMAGE: u8 = 0x98;
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
    /// PA clamping threshold (datasheet errata 15.2)
    pub const TX_CLAMP_CONFIG: u16 = 0x08D8;
}

/// Maximum payload length the modem can carry in one packet.
///
/// The packet-params length field is a single byte, so the hardware limit is
/// 255 even though `MAX_LORA_PAYLOAD` is 256. Used both to bound RX and to
/// reject oversize TX payloads (256 would otherwise truncate to 0 and
/// silently transmit an empty packet).
const HW_MAX_PAYLOAD_LEN: u8 = if MAX_LORA_PAYLOAD > 255 {
    255
} else {
    MAX_LORA_PAYLOAD as u8
};

/// Standby modes
mod standby {
    pub const STDBY_RC: u8 = 0x00;
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
    Dio1: InputPin + Wait,
    Nrst: OutputPin,
    Busy: InputPin,
{
    spi: Spi,
    nss: Nss,
    dio1: Dio1,
    nrst: Nrst,
    busy: Busy,
    initialised: bool,
    in_continuous_rx: bool,
    config: Option<LoraConfig>,
}

impl<Spi, Nss, Dio1, Nrst, Busy> Sx1262Driver<Spi, Nss, Dio1, Nrst, Busy>
where
    Spi: SpiBus,
    Nss: OutputPin,
    Dio1: InputPin + Wait,
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
            in_continuous_rx: false,
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
        // Every SX1262 command used here fits in 15 data bytes; reject longer
        // input rather than silently truncating it into a corrupt command.
        if data.len() > 15 {
            return Err(LoraError::InvalidConfig);
        }

        self.wait_not_busy().await?;

        let _ = self.nss.set_low();

        let mut buf = [0u8; 16];
        buf[0] = cmd;
        let len = 1 + data.len();
        buf[1..len].copy_from_slice(data);

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

    /// Select the DC-DC regulator (must be issued in STDBY_RC).
    ///
    /// The Wio-SX1262 module is built with the DC-DC supply option, but the
    /// chip powers up in LDO-only mode, which nearly doubles RX/TX current.
    async fn set_regulator_mode_dcdc(&mut self) -> Result<(), LoraError> {
        self.write_command(cmd::SET_REGULATOR_MODE, &[0x01]).await
    }

    /// Clear the device error flags.
    ///
    /// XOSC_START_ERR is always raised at POR when a TCXO is used (the chip
    /// is not yet aware it is clocked by one); the datasheet says to simply
    /// clear it.
    async fn clear_device_errors(&mut self) -> Result<(), LoraError> {
        self.write_command(cmd::CLEAR_DEVICE_ERRORS, &[0x00, 0x00])
            .await
    }

    /// Read a single register.
    async fn read_register(&mut self, addr: u16) -> Result<u8, LoraError> {
        self.wait_not_busy().await?;

        let _ = self.nss.set_low();

        // Opcode + address (2) + status NOP, then the data byte clocks out.
        let tx_buf = [cmd::READ_REGISTER, (addr >> 8) as u8, addr as u8, 0x00, 0x00];
        let mut rx_buf = [0u8; 5];

        let result = self
            .spi
            .transfer(&mut rx_buf, &tx_buf)
            .await
            .map_err(|_| LoraError::SpiError);

        let _ = self.nss.set_high();

        result?;
        Ok(rx_buf[4])
    }

    /// Raise the PA clamping threshold (datasheet errata 15.2).
    ///
    /// Without this the SX1262 backs its output power off by typically
    /// 5-6 dB into any reasonable antenna mismatch. Bits 4:1 of
    /// TxClampConfig must be set to 1111 after every POR / cold start.
    async fn apply_tx_clamp_workaround(&mut self) -> Result<(), LoraError> {
        let value = self.read_register(reg::TX_CLAMP_CONFIG).await?;
        self.write_register(reg::TX_CLAMP_CONFIG, value | 0x1E).await
    }

    /// Recalibrate all blocks.
    ///
    /// Required after enabling the TCXO: the power-on calibration ran from the
    /// RC oscillator before the TCXO was available, so the radio must be
    /// recalibrated or the first TX/RX uses an invalid calibration.
    async fn calibrate_all(&mut self) -> Result<(), LoraError> {
        self.write_command(cmd::CALIBRATE, &[CALIBRATE_ALL]).await?;
        // Calibration drives BUSY high while it runs; allow it to settle.
        Timer::after(Duration::from_millis(5)).await;
        self.wait_not_busy().await
    }

    /// Calibrate image rejection for the configured frequency band.
    async fn calibrate_image(&mut self, freq_hz: u32) -> Result<(), LoraError> {
        let (f1, f2) = image_cal_params(freq_hz);
        self.write_command(cmd::CALIBRATE_IMAGE, &[f1, f2]).await
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

    /// Set standby mode.
    ///
    /// Clears `in_continuous_rx` so every path that drops the radio out of
    /// RX (transmit, configure, set_standby) leaves the flag truthful and
    /// the next `arm_receive` re-arms the receiver.
    async fn set_standby_internal(&mut self) -> Result<(), LoraError> {
        self.in_continuous_rx = false;
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

        // Low data rate optimisation: required when the symbol time reaches
        // 16.38 ms (SF11/SF12 at 125 kHz and SF12 at 250 kHz).
        let ldro = if ldro_enabled(config.spreading_factor, config.bandwidth_khz) {
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

    /// Wait for DIO1 to signal an interrupt, without touching the SPI bus.
    ///
    /// Cancel-safe: only the GPIO interrupt and a timer are awaited, so this
    /// future can be raced in a `select` and dropped at any point without
    /// leaving the radio or the SPI bus in a half-driven state.
    async fn wait_dio1(&mut self, timeout_ms: u32) -> Result<(), LoraError> {
        match with_timeout(
            Duration::from_millis(timeout_ms as u64),
            self.dio1.wait_for_high(),
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            // Pin errors are unreachable on esp-hal (Infallible).
            Ok(Err(_)) => Err(LoraError::SpiError),
            Err(_) => Err(LoraError::Timeout),
        }
    }

    /// Wait for DIO1 interrupt with timeout, then read the IRQ status
    async fn wait_for_irq(&mut self, timeout_ms: u32) -> Result<u16, LoraError> {
        self.wait_dio1(timeout_ms).await?;
        self.get_irq_status().await
    }

    /// Start continuous receive mode (like Arduino's startReceive)
    /// Puts the radio into RX mode with no timeout
    async fn start_receive_mode(&mut self) -> Result<(), LoraError> {
        // Set to standby first
        self.set_standby_internal().await?;

        // Set packet parameters for max length
        self.set_packet_params(HW_MAX_PAYLOAD_LEN).await?;

        // Configure IRQ for RX done, timeout, CRC error
        self.configure_irq(irq::RX_DONE | irq::TIMEOUT | irq::CRC_ERR)
            .await?;
        self.clear_irq(0xFFFF).await?;

        // Start continuous RX (timeout = 0xFFFFFF means continuous)
        let timeout_bytes = [0xFF, 0xFF, 0xFF];
        self.write_command(cmd::SET_RX, &timeout_bytes).await?;

        self.in_continuous_rx = true;
        Ok(())
    }
}

impl<Spi, Nss, Dio1, Nrst, Busy> LoraRadio for Sx1262Driver<Spi, Nss, Dio1, Nrst, Busy>
where
    Spi: SpiBus,
    Nss: OutputPin,
    Dio1: InputPin + Wait,
    Nrst: OutputPin,
    Busy: InputPin,
{
    async fn init(&mut self) -> Result<(), LoraError> {
        // Reset the radio
        self.reset().await?;
        self.wait_not_busy().await?;

        // Set standby mode
        self.set_standby_internal().await?;

        // The Wio-SX1262 is a DC-DC design; must be selected in STDBY_RC
        self.set_regulator_mode_dcdc().await?;

        // Configure TCXO (1.8V)
        self.configure_tcxo().await?;
        Timer::after(Duration::from_millis(10)).await;

        // Recalibrate now the TCXO is running (must be done in STDBY_RC).
        // Without this the first TX/RX after boot uses the invalid power-on
        // calibration and the first packet is silently lost.
        self.calibrate_all().await?;

        // The XOSC start triggers the expected XOSC_START_ERR flag when a
        // TCXO is used; the datasheet says to clear it and move on.
        self.clear_device_errors().await?;

        // PA clamp errata fix; required once per POR / cold start
        self.apply_tx_clamp_workaround().await?;

        // Configure DIO2 as RF switch control
        self.configure_dio2_rf_switch().await?;

        // Set packet type to LoRa
        self.set_packet_type_lora().await?;

        // Set buffer base addresses. TX and RX never run concurrently, so
        // both use the full 256-byte buffer; splitting it capped RX at 128
        // bytes before wrapping.
        self.set_buffer_base_address(0x00, 0x00).await?;

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

        // The modem's length field is one byte: reject anything over 255, or
        // the length wraps (256 -> 0) and an empty packet goes on the air.
        if data.is_empty() || data.len() > HW_MAX_PAYLOAD_LEN as usize {
            return Err(LoraError::InvalidConfig);
        }

        // Set to standby (clears in_continuous_rx); arm_receive re-arms
        // continuous RX afterwards.
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

        // Clear IRQ after reading (Semtech pattern)
        self.clear_irq(irq_status).await?;

        // NOTE: Don't call start_receive_mode() here.
        // The lora_task will call arm_receive() which handles RX mode re-entry.

        if irq_status & irq::TX_DONE != 0 {
            Ok(())
        } else {
            Err(LoraError::TransmitFailed)
        }
    }

    async fn arm_receive(&mut self) -> Result<(), LoraError> {
        if !self.initialised {
            return Err(LoraError::NotInitialised);
        }

        // Arm continuous RX only when the radio is not already listening
        // (after boot or a transmit). Re-arming on every poll used to drop
        // the radio to standby mid-reception, which deterministically killed
        // any packet whose airtime outlived the poll window - at SF11 that
        // was everything much bigger than a few tens of bytes.
        if !self.in_continuous_rx {
            self.start_receive_mode().await?;
        }
        Ok(())
    }

    async fn wait_rx_event(&mut self, timeout_ms: u32) -> Result<(), LoraError> {
        // Pure GPIO/timer wait - no SPI - so the caller may race this in a
        // select and drop it at any await point. On deadline return Timeout
        // WITHOUT touching the radio: a reception may be in flight and a
        // later poll picks up the completed packet.
        self.wait_dio1(timeout_ms).await
    }

    async fn read_packet(&mut self) -> Result<RxPacket, LoraError> {
        // Read then clear the IRQ (Semtech pattern: read -> clear -> process)
        let irq_status = self.get_irq_status().await?;
        self.clear_irq(irq_status).await?;

        // The radio stays in continuous RX after these outcomes.
        if irq_status & irq::CRC_ERR != 0 {
            return Err(LoraError::CrcError);
        }
        if irq_status & irq::RX_DONE == 0 {
            return Err(LoraError::ReceiveFailed);
        }

        // Read the packet; continuous RX keeps listening for the next one.
        let (payload_len, buffer_offset) = self.get_rx_buffer_status().await?;
        let data = self.read_buffer(buffer_offset, payload_len as usize).await?;
        let (rssi, snr) = self.get_packet_status().await?;

        Ok(RxPacket { data, rssi, snr })
    }

    async fn configure(&mut self, config: &LoraConfig) -> Result<(), LoraError> {
        // Set to standby before configuration
        self.set_standby_internal().await?;

        // Set frequency
        self.set_frequency(config.frequency_hz).await?;

        // Calibrate image rejection for this frequency band
        self.calibrate_image(config.frequency_hz).await?;

        // Set modulation parameters
        self.set_modulation_params(config).await?;

        // Configure Power Amplifier (must be called before SetTxParams)
        self.configure_pa().await?;

        // SetPaConfig resets the OCP register to its default, so the current
        // limit must be (re)written after it, never before.
        self.set_current_limit(140).await?;

        // Set TX power
        self.set_tx_power(config.tx_power_dbm).await?;

        self.config = Some(config.clone());

        Ok(())
    }

    async fn set_standby(&mut self) -> Result<(), LoraError> {
        self.set_standby_internal().await
    }
}

#[cfg(all(test, feature = "host-test"))]
mod host_tests {
    //! Host-side tests that drive the real driver against a recording SPI bus.
    //!
    //! Run with: `cargo test --target x86_64-unknown-linux-gnu --features host-test`

    use super::*;
    use crate::lora::traits::LoraRadio;
    use core::cell::RefCell;
    use core::future::Future;
    use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    use std::rc::Rc;
    use std::vec::Vec as StdVec;

    /// Error type for the mocks (never actually returned).
    #[derive(Debug)]
    struct MockError;

    impl embedded_hal::spi::Error for MockError {
        fn kind(&self) -> embedded_hal::spi::ErrorKind {
            embedded_hal::spi::ErrorKind::Other
        }
    }

    impl embedded_hal::digital::Error for MockError {
        fn kind(&self) -> embedded_hal::digital::ErrorKind {
            embedded_hal::digital::ErrorKind::Other
        }
    }

    /// SPI bus that records every outbound buffer (first byte is the opcode).
    #[derive(Clone)]
    struct RecordingSpi {
        writes: Rc<RefCell<StdVec<StdVec<u8>>>>,
    }

    impl embedded_hal::spi::ErrorType for RecordingSpi {
        type Error = MockError;
    }

    impl embedded_hal_async::spi::SpiBus<u8> for RecordingSpi {
        async fn read(&mut self, _words: &mut [u8]) -> Result<(), MockError> {
            Ok(())
        }

        async fn write(&mut self, words: &[u8]) -> Result<(), MockError> {
            self.writes.borrow_mut().push(words.to_vec());
            Ok(())
        }

        async fn transfer(&mut self, read: &mut [u8], write: &[u8]) -> Result<(), MockError> {
            self.writes.borrow_mut().push(write.to_vec());
            read.iter_mut().for_each(|b| *b = 0);
            Ok(())
        }

        async fn transfer_in_place(&mut self, _words: &mut [u8]) -> Result<(), MockError> {
            Ok(())
        }

        async fn flush(&mut self) -> Result<(), MockError> {
            Ok(())
        }
    }

    /// Output pin that ignores writes.
    struct NoopOut;
    impl embedded_hal::digital::ErrorType for NoopOut {
        type Error = MockError;
    }
    impl embedded_hal::digital::OutputPin for NoopOut {
        fn set_low(&mut self) -> Result<(), MockError> {
            Ok(())
        }
        fn set_high(&mut self) -> Result<(), MockError> {
            Ok(())
        }
    }

    /// Input pin that always reads low (BUSY released, no IRQ pending).
    struct LowPin;
    impl embedded_hal::digital::ErrorType for LowPin {
        type Error = MockError;
    }
    impl embedded_hal::digital::InputPin for LowPin {
        fn is_high(&mut self) -> Result<bool, MockError> {
            Ok(false)
        }
        fn is_low(&mut self) -> Result<bool, MockError> {
            Ok(true)
        }
    }
    impl embedded_hal_async::digital::Wait for LowPin {
        async fn wait_for_high(&mut self) -> Result<(), MockError> {
            core::future::pending().await
        }
        async fn wait_for_low(&mut self) -> Result<(), MockError> {
            Ok(())
        }
        async fn wait_for_rising_edge(&mut self) -> Result<(), MockError> {
            core::future::pending().await
        }
        async fn wait_for_falling_edge(&mut self) -> Result<(), MockError> {
            core::future::pending().await
        }
        async fn wait_for_any_edge(&mut self) -> Result<(), MockError> {
            core::future::pending().await
        }
    }

    /// Minimal no-op waker built from core only (avoids extra deps).
    fn noop_waker() -> Waker {
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(core::ptr::null(), &VTABLE)
        }
        fn noop(_: *const ()) {}
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
    }

    /// Drive a future to completion, advancing the mock clock past any timers.
    fn run<F: Future>(fut: F) -> F::Output {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut fut = core::pin::pin!(fut);
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => {
                    embassy_time::MockDriver::get().advance(Duration::from_millis(50));
                }
            }
        }
    }

    fn build_driver(
        writes: Rc<RefCell<StdVec<StdVec<u8>>>>,
    ) -> Sx1262Driver<RecordingSpi, NoopOut, LowPin, NoopOut, LowPin> {
        let spi = RecordingSpi { writes };
        Sx1262Driver::new(
            spi,
            Sx1262Pins {
                nss: NoopOut,
                dio1: LowPin,
                nrst: NoopOut,
                busy: LowPin,
            },
        )
    }

    fn first_index(writes: &[StdVec<u8>], opcode: u8) -> Option<usize> {
        writes.iter().position(|w| w.first() == Some(&opcode))
    }

    #[test]
    fn init_calibrates_after_tcxo_and_image_after_frequency() {
        embassy_time::MockDriver::get().reset();
        let writes = Rc::new(RefCell::new(StdVec::new()));
        let mut driver = build_driver(writes.clone());

        run(driver.init()).expect("init should succeed");

        let writes = writes.borrow();

        let tcxo = first_index(&writes, cmd::SET_DIO3_AS_TCXO_CTRL)
            .expect("TCXO control should be configured");
        let calibrate =
            first_index(&writes, cmd::CALIBRATE).expect("Calibrate must be issued during init");
        assert!(
            calibrate > tcxo,
            "Calibrate (0x89) must be issued after SetDIO3AsTcxoCtrl (0x97)"
        );
        assert_eq!(
            writes[calibrate].get(1).copied(),
            Some(CALIBRATE_ALL),
            "Calibrate must request all blocks (0x7F)"
        );

        let frequency =
            first_index(&writes, cmd::SET_RF_FREQUENCY).expect("RF frequency should be set");
        let image = first_index(&writes, cmd::CALIBRATE_IMAGE)
            .expect("CalibrateImage must be issued during init");
        assert!(
            image > frequency,
            "CalibrateImage (0x98) must be issued after SetRfFrequency (0x86)"
        );
        assert_eq!(
            &writes[image][1..3],
            &[0xD7, 0xDB],
            "Default 869.525 MHz maps to the 863-870 MHz image band"
        );
    }

    #[test]
    fn calibrate_image_emits_band_bytes() {
        let writes = Rc::new(RefCell::new(StdVec::new()));
        let mut driver = build_driver(writes.clone());

        // calibrate_image takes no timer path, so a plain executor is enough.
        run(driver.calibrate_image(915_000_000)).expect("calibrate_image should succeed");

        let writes = writes.borrow();
        let image =
            first_index(&writes, cmd::CALIBRATE_IMAGE).expect("CalibrateImage should be recorded");
        assert_eq!(&writes[image][1..3], &[0xE1, 0xE9]);
    }

    /// Find the first WriteRegister (0x0D) targeting the given address.
    fn first_register_write(writes: &[StdVec<u8>], addr: u16) -> Option<usize> {
        writes.iter().position(|w| {
            w.first() == Some(&cmd::WRITE_REGISTER)
                && w.get(1) == Some(&((addr >> 8) as u8))
                && w.get(2) == Some(&(addr as u8))
        })
    }

    #[test]
    fn init_sets_regulator_clamp_errors_and_ocp_order() {
        embassy_time::MockDriver::get().reset();
        let writes = Rc::new(RefCell::new(StdVec::new()));
        let mut driver = build_driver(writes.clone());

        run(driver.init()).expect("init should succeed");

        let writes = writes.borrow();

        // DC-DC regulator selected (module is a DC-DC design), in STDBY_RC.
        let standby = first_index(&writes, cmd::SET_STANDBY).expect("standby issued");
        let regulator = first_index(&writes, cmd::SET_REGULATOR_MODE)
            .expect("SetRegulatorMode must be issued during init");
        assert_eq!(writes[regulator].get(1), Some(&0x01), "DC_DC+LDO mode");
        assert!(regulator > standby, "regulator mode must follow standby");

        // XOSC_START_ERR cleared after the TCXO-driven recalibration.
        let calibrate = first_index(&writes, cmd::CALIBRATE).expect("calibrate issued");
        let clear_errors = first_index(&writes, cmd::CLEAR_DEVICE_ERRORS)
            .expect("ClearDeviceErrors must be issued during init");
        assert!(clear_errors > calibrate);

        // Errata 15.2: TxClampConfig bits 4:1 set after read-modify-write.
        let clamp = first_register_write(&writes, reg::TX_CLAMP_CONFIG)
            .expect("TxClampConfig must be written during init");
        let clamp_value = writes[clamp][3];
        assert_eq!(clamp_value & 0x1E, 0x1E, "PA clamp bits 4:1 must be 1111");

        // OCP is reset by SetPaConfig, so the explicit limit must come after.
        let pa_config = first_index(&writes, cmd::SET_PA_CONFIG).expect("PA configured");
        let ocp = first_register_write(&writes, reg::OCP_CONFIGURATION)
            .expect("OCP limit must be written during init");
        assert!(
            ocp > pa_config,
            "OCP write (reg 0x08E7) must follow SetPaConfig, which resets it"
        );
    }

    #[test]
    fn init_applies_sf11_default() {
        embassy_time::MockDriver::get().reset();
        let writes = Rc::new(RefCell::new(StdVec::new()));
        let mut driver = build_driver(writes.clone());

        run(driver.init()).expect("init should succeed");

        let writes = writes.borrow();
        let modulation = first_index(&writes, cmd::SET_MODULATION_PARAMS)
            .expect("modulation params must be set during init");
        // [opcode, SF, BW, CR, LDRO]
        assert_eq!(writes[modulation][1], 11, "default must be SF11");
        assert_eq!(writes[modulation][2], 0x05, "250 kHz bandwidth code");
        assert_eq!(writes[modulation][4], 0x00, "SF11 at 250 kHz needs no LDRO");
    }

    #[test]
    fn transmit_rejects_payload_over_255_bytes() {
        embassy_time::MockDriver::get().reset();
        let writes = Rc::new(RefCell::new(StdVec::new()));
        let mut driver = build_driver(writes.clone());

        run(driver.init()).expect("init should succeed");

        // 256 bytes fits MAX_LORA_PAYLOAD but overflows the modem's one-byte
        // length field; it used to truncate to 0 and transmit an empty packet.
        let payload = [0xAAu8; 256];
        let result = run(driver.transmit(&payload));
        assert_eq!(result, Err(LoraError::InvalidConfig));

        let writes = writes.borrow();
        assert!(
            first_index(&writes, cmd::SET_TX).is_none(),
            "an oversize payload must never reach SetTx"
        );
    }

    #[test]
    fn transmit_accepts_255_byte_payload() {
        embassy_time::MockDriver::get().reset();
        let writes = Rc::new(RefCell::new(StdVec::new()));
        let mut driver = build_driver(writes.clone());

        run(driver.init()).expect("init should succeed");

        // The mock DIO1 never fires, so a valid transmit runs until the IRQ
        // wait times out - proving it passed validation and hit the radio.
        let payload = [0x55u8; 255];
        let result = run(driver.transmit(&payload));
        assert_eq!(result, Err(LoraError::Timeout));

        let writes = writes.borrow();
        let set_tx = first_index(&writes, cmd::SET_TX);
        assert!(set_tx.is_some(), "a 255-byte payload must reach SetTx");
        let params = writes
            .iter()
            .rposition(|w| w.first() == Some(&cmd::SET_PACKET_PARAMS))
            .expect("packet params written for TX");
        assert_eq!(writes[params][4], 255, "payload length must survive intact");
    }

    #[test]
    fn standby_clears_continuous_rx_and_arm_rearms() {
        embassy_time::MockDriver::get().reset();
        let writes = Rc::new(RefCell::new(StdVec::new()));
        let mut driver = build_driver(writes.clone());

        run(driver.init()).expect("init should succeed");
        assert!(driver.in_continuous_rx, "init leaves the radio listening");

        run(driver.set_standby()).expect("standby should succeed");
        assert!(
            !driver.in_continuous_rx,
            "any path through standby must clear the continuous-RX flag"
        );

        let rx_before = writes.borrow().iter().filter(|w| w.first() == Some(&cmd::SET_RX)).count();
        run(driver.arm_receive()).expect("arm_receive should succeed");
        assert!(driver.in_continuous_rx);
        let rx_after = writes.borrow().iter().filter(|w| w.first() == Some(&cmd::SET_RX)).count();
        assert_eq!(rx_after, rx_before + 1, "arm_receive must re-issue SetRx");
    }

    #[test]
    fn wait_rx_event_times_out_without_spi_traffic() {
        embassy_time::MockDriver::get().reset();
        let writes = Rc::new(RefCell::new(StdVec::new()));
        let mut driver = build_driver(writes.clone());

        run(driver.init()).expect("init should succeed");
        let writes_before = writes.borrow().len();

        let result = run(driver.wait_rx_event(100));
        assert_eq!(result, Err(LoraError::Timeout));
        assert_eq!(
            writes.borrow().len(),
            writes_before,
            "the cancel-safe wait must not touch the SPI bus"
        );
    }
}
