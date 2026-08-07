//! WiFi station connection manager and IP watcher.
//!
//! The connection task keeps the station associated with exponential
//! backoff on failures; the watcher task mirrors the DHCP state into
//! [`HUB_STATS`] so `GetHubStatus` reports the live IP.

use embassy_net::Stack;
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{
    AuthMethod, ClientConfig, ModeConfig, WifiController, WifiEvent, WifiStaState,
};
use hub_protocol::LinkState;

use crate::debug;
use crate::hub_config::HubConfig;
use crate::net::backoff::Backoff;
use crate::net::stats::HUB_STATS;

/// Keeps the WiFi station associated to the configured network.
pub async fn wifi_task(mut controller: WifiController<'static>, config: &'static HubConfig) {
    if !config.wifi_provisioned() {
        HUB_STATS.set_wifi_state(LinkState::Unprovisioned);
        debug!("WiFi: no SSID configured; provision over USB serial");
        core::future::pending::<()>().await;
    }

    let mut backoff = Backoff::new();
    loop {
        if matches!(esp_radio::wifi::sta_state(), WifiStaState::Connected) {
            backoff.reset();
            HUB_STATS.set_wifi_state(LinkState::Connected);
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            HUB_STATS.set_wifi_state(LinkState::Disconnected);
            debug!("WiFi: disconnected");
            continue;
        }

        if !matches!(controller.is_started(), Ok(true)) {
            if let Err(err) = start_station(&mut controller, config).await {
                debug!("WiFi: start failed: {:?}", err);
                Timer::after(Duration::from_secs(backoff.next_secs() as u64)).await;
                continue;
            }
        }

        HUB_STATS.set_wifi_state(LinkState::Connecting);
        debug!("WiFi: connecting to {}", config.wifi.ssid.as_str());
        match controller.connect_async().await {
            Ok(()) => debug!("WiFi: associated"),
            Err(err) => {
                HUB_STATS.set_wifi_state(LinkState::Disconnected);
                debug!("WiFi: connect failed: {:?}", err);
                Timer::after(Duration::from_secs(backoff.next_secs() as u64)).await;
            }
        }
    }
}

async fn start_station(
    controller: &mut WifiController<'static>,
    config: &HubConfig,
) -> Result<(), esp_radio::wifi::WifiError> {
    let mut client = ClientConfig::default()
        .with_ssid(config.wifi.ssid.as_str().into())
        .with_password(config.wifi.password.as_str().into());
    if config.wifi.password.is_empty() {
        client = client.with_auth_method(AuthMethod::None);
    }
    controller.set_config(&ModeConfig::Client(client))?;
    controller.start_async().await
}

/// Mirrors DHCP config changes into the stats the host can query.
pub async fn net_watch_task(stack: Stack<'static>) {
    loop {
        stack.wait_config_up().await;
        if let Some(config) = stack.config_v4() {
            let ip = config.address.address().octets();
            HUB_STATS.set_ip(ip);
            debug!("WiFi: IP {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        }
        stack.wait_config_down().await;
        HUB_STATS.clear_ip();
        debug!("WiFi: lost IP configuration");
    }
}
