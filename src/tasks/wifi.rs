//! WiFi manager: station connection with backoff, and the provisioning
//! access point.
//!
//! Unprovisioned hubs (no SSID) start straight in SoftAP mode so a phone
//! can join `WalkieTextieHub-XXXXXX` and configure them. Provisioned hubs
//! run as a station; if the network stays unreachable through repeated
//! attempts (wrong password, network gone) the hub falls back to the
//! access point so it can be re-provisioned without a serial cable.
//! The IP watcher mirrors DHCP state into [`HUB_STATS`] so `GetHubStatus`
//! reports the live address.

use embassy_net::Stack;
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{
    AccessPointConfig, AuthMethod, ClientConfig, ModeConfig, WifiController, WifiEvent,
    WifiStaState,
};
use hub_protocol::LinkState;

use crate::debug;
use crate::hub_config::HubConfig;
use crate::net::backoff::Backoff;
use crate::net::stats::HUB_STATS;

/// Consecutive station failures before falling back to the provisioning AP
/// (with exponential backoff this spans several minutes).
const MAX_STA_FAILURES: u32 = 10;

/// Keeps the WiFi station associated, or runs the provisioning AP.
pub async fn wifi_task(
    mut controller: WifiController<'static>,
    config: &'static HubConfig,
    device_id: [u8; 3],
) {
    if !config.wifi_provisioned() {
        HUB_STATS.set_wifi_state(LinkState::Unprovisioned);
        debug!("WiFi: no SSID configured, starting provisioning AP");
        run_access_point(&mut controller, device_id).await;
        return;
    }

    let mut backoff = Backoff::new();
    let mut failures = 0u32;
    loop {
        if failures >= MAX_STA_FAILURES {
            HUB_STATS.set_wifi_state(LinkState::Unprovisioned);
            debug!("WiFi: giving up on the station, starting provisioning AP");
            let _ = controller.stop_async().await;
            run_access_point(&mut controller, device_id).await;
            return;
        }

        if matches!(esp_radio::wifi::sta_state(), WifiStaState::Connected) {
            backoff.reset();
            failures = 0;
            HUB_STATS.set_wifi_state(LinkState::Connected);
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            HUB_STATS.set_wifi_state(LinkState::Disconnected);
            debug!("WiFi: disconnected");
            continue;
        }

        if !matches!(controller.is_started(), Ok(true)) {
            if let Err(err) = start_station(&mut controller, config).await {
                debug!("WiFi: start failed: {:?}", err);
                failures += 1;
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
                failures += 1;
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

/// Start the open provisioning access point and keep it up. The portal
/// tasks notice via the AP stack's link coming up.
async fn run_access_point(controller: &mut WifiController<'static>, device_id: [u8; 3]) {
    let mut ssid: heapless::String<32> = heapless::String::new();
    let _ = ssid.push_str("WalkieTextieHub-");
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in device_id {
        let _ = ssid.push(HEX[(byte >> 4) as usize] as char);
        let _ = ssid.push(HEX[(byte & 0x0F) as usize] as char);
    }

    let ap = AccessPointConfig::default()
        .with_ssid(ssid.as_str().into())
        .with_auth_method(AuthMethod::None);
    if let Err(err) = controller.set_config(&ModeConfig::AccessPoint(ap)) {
        debug!("WiFi: AP config failed: {:?}", err);
        return;
    }
    match controller.start_async().await {
        Ok(()) => debug!("WiFi: provisioning AP '{}' up at 192.168.4.1", ssid.as_str()),
        Err(err) => {
            debug!("WiFi: AP start failed: {:?}", err);
            return;
        }
    }
    core::future::pending::<()>().await;
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
