//! Gateway radio task: single-channel LoRaWAN uplink RX and timed downlink TX.
//!
//! Owns the SX1262 in gateway mode. Listens on the configured channel with
//! the public sync word; downlink jobs from the UDP task are executed at
//! their target microsecond counter value using the driver's split TX
//! (prepare ahead of the window, fire on the instant).

use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal_async::digital::Wait;
use embedded_hal_async::spi::SpiBus;

use crate::debug;
use crate::gateway::scheduler::{tmst_delta_us, SETUP_LEAD_US, TX_START_COMP_US};
use crate::gateway::udp_protocol::RxMeta;
use crate::gateway::{DownlinkJob, GW_UPLINK_CHANNEL};
use crate::hub_config::HubConfig;
use crate::lora::driver::Sx1262Driver;
use crate::lora::traits::{LoraConfig, LoraRadio, SyncWord};
use crate::net::stats::HUB_STATS;
use crate::tasks::led::LedFlashDuration;
use crate::tasks::LedSender;

/// Idle poll interval for the cancel-safe RX wait.
const RX_POLL_INTERVAL_MS: u32 = 500;

/// The uplink listen configuration for the gateway channel.
fn uplink_config(config: &HubConfig) -> LoraConfig {
    LoraConfig {
        frequency_hz: config.gateway.frequency_hz,
        spreading_factor: config.gateway.spreading_factor,
        bandwidth_khz: config.gateway.bandwidth_khz,
        coding_rate: config.gateway.coding_rate,
        sync_word: SyncWord::Public,
        iq_inverted: false,
        crc_on: true,
        preamble_len: 8,
        ..LoraConfig::default()
    }
}

pub async fn gateway_task<Spi, Nss, Dio1, Nrst, Busy>(
    mut radio: Sx1262Driver<Spi, Nss, Dio1, Nrst, Busy>,
    config: &'static HubConfig,
    led_sender: LedSender,
) where
    Spi: SpiBus,
    Nss: OutputPin,
    Dio1: InputPin + Wait,
    Nrst: OutputPin,
    Busy: InputPin,
{
    if let Err(err) = radio.init().await {
        debug!("Gateway: radio init failed: {:?}", err);
        return;
    }
    let uplink = uplink_config(config);
    if let Err(err) = radio.configure(&uplink).await {
        debug!("Gateway: radio configure failed: {:?}", err);
        return;
    }
    debug!(
        "Gateway: listening on {} Hz SF{}BW{}",
        uplink.frequency_hz, uplink.spreading_factor, uplink.bandwidth_khz
    );

    let downlink_receiver = crate::gateway::GW_DOWNLINK_CHANNEL.receiver();
    let mut pending: Option<DownlinkJob> = None;

    loop {
        if let Err(err) = radio.arm_receive().await {
            debug!("Gateway: arm_receive failed: {:?}", err);
            Timer::after(Duration::from_millis(500)).await;
            continue;
        }

        if let Some(job) = pending.take() {
            execute_downlink(&mut radio, &job, &led_sender).await;
            if let Err(err) = radio.configure(&uplink).await {
                debug!("Gateway: uplink reconfigure failed: {:?}", err);
            }
            continue;
        }

        // Only the cancel-safe wait may be raced here (same discipline as
        // the bridge-mode lora task).
        match select(
            radio.wait_rx_event(RX_POLL_INTERVAL_MS),
            downlink_receiver.receive(),
        )
        .await
        {
            Either::First(Ok(rx_instant)) => {
                let Ok(packet) = radio.read_packet().await else {
                    // CRC errors and spurious IRQs; the radio stays in RX.
                    continue;
                };
                let _ = led_sender.try_send(LedFlashDuration::Default);
                HUB_STATS.count_uplink();
                debug!(
                    "Gateway RX: {} bytes (RSSI: {}, SNR: {})",
                    packet.data.len(),
                    packet.rssi,
                    packet.snr
                );
                let meta = RxMeta {
                    tmst: rx_instant.as_micros() as u32,
                    freq_hz: uplink.frequency_hz,
                    spreading_factor: uplink.spreading_factor,
                    bandwidth_khz: uplink.bandwidth_khz,
                    coding_rate: uplink.coding_rate,
                    rssi: packet.rssi,
                    snr: packet.snr,
                };
                if GW_UPLINK_CHANNEL.try_send((meta, packet.data)).is_err() {
                    HUB_STATS.count_dropped(1);
                }
            }
            // Timeout is the normal idle case; re-loop and re-arm.
            Either::First(Err(_)) => {}
            Either::Second(job) => {
                pending = Some(job);
            }
        }
    }
}

/// Transmit one downlink at its target time (or immediately for Class C).
///
/// The radio leaves continuous RX for the duration - a single half-duplex
/// SX1262 cannot hear uplinks while transmitting.
async fn execute_downlink<Spi, Nss, Dio1, Nrst, Busy>(
    radio: &mut Sx1262Driver<Spi, Nss, Dio1, Nrst, Busy>,
    job: &DownlinkJob,
    led_sender: &LedSender,
) where
    Spi: SpiBus,
    Nss: OutputPin,
    Dio1: InputPin + Wait,
    Nrst: OutputPin,
    Busy: InputPin,
{
    if let Some(target) = job.target_tmst {
        let now = Instant::now();
        let delta = tmst_delta_us(now.as_micros() as u32, target);
        if delta < SETUP_LEAD_US as i32 {
            debug!("Gateway TX: window already passed, dropping downlink");
            return;
        }
        // Sleep until setup time, then stage everything except SetTx.
        Timer::at(now + Duration::from_micros((delta as u32 - SETUP_LEAD_US) as u64)).await;
    }

    if let Err(err) = radio.configure(&job.config).await {
        debug!("Gateway TX: configure failed: {:?}", err);
        return;
    }
    if let Err(err) = radio.prepare_tx(&job.payload).await {
        debug!("Gateway TX: prepare failed: {:?}", err);
        return;
    }

    if let Some(target) = job.target_tmst {
        // Re-take the clock after the SPI setup and fire just early enough
        // to compensate the SetTx latency and PA ramp.
        let now = Instant::now();
        let remaining = tmst_delta_us(now.as_micros() as u32, target);
        if remaining > TX_START_COMP_US as i32 {
            Timer::at(now + Duration::from_micros((remaining as u32 - TX_START_COMP_US) as u64))
                .await;
        }
    }

    if let Err(err) = radio.fire_tx().await {
        debug!("Gateway TX: fire failed: {:?}", err);
        return;
    }
    match radio.wait_tx_done().await {
        Ok(()) => {
            let _ = led_sender.try_send(LedFlashDuration::Default);
            HUB_STATS.count_downlink();
            debug!("Gateway TX: {} bytes sent", job.payload.len());
        }
        Err(err) => debug!("Gateway TX: failed: {:?}", err),
    }
}
